//! Shake compaction — mechanical (LLM-free) context compression.
//!
//! Walks the message log backwards to identify a recent
//! [`protect_window_tokens`](ShakeConfig::protect_window_tokens) "tail" that
//! must be preserved verbatim, then elides large token-heavy regions from
//! everything older:
//!
//! 1. Tool result messages whose text payload is at least
//!    [`min_elidable_tokens`](ShakeConfig::min_elidable_tokens).
//! 2. Fenced code blocks (`` ```...``` ``) of at least the same size,
//!    inside any message text.
//!
//! Each region is replaced with a compact placeholder. If the total
//! recovered tokens meet [`min_savings_tokens`](ShakeConfig::min_savings_tokens),
//! every region is elided in a single pass and the function reports
//! [`ShakeOutcome::Shaken`]. Otherwise **no message is mutated** and the
//! function reports [`ShakeOutcome::NoChange`] — callers can poll the same
//! vector repeatedly without side effects.
//!
//! Ported from omp `packages/agent/src/compaction/shake.ts` (mechanical,
//! regex-free analogue). Token counts use the `chars / 4` heuristic that
//! the rest of the agent loop already uses for cold-start estimation.

use oxicode_ai::{ContentBlock, Message, MessageContent, TextContent};

/// Tunable thresholds for [`shake`].
///
/// Defaults mirror omp's reference implementation:
/// `protect_window_tokens = 16384`, `min_elidable_tokens = 400`,
/// `min_savings_tokens = 4096`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShakeConfig {
    /// Number of most-recent tokens that must remain untouched. The
    /// boundary is inclusive: messages from the tail that, together,
    /// cover at least this many tokens are protected.
    pub protect_window_tokens: usize,
    /// Minimum token count for a region to be considered for elision.
    /// Smaller regions are left alone to avoid noisy churn.
    pub min_elidable_tokens: usize,
    /// Minimum aggregate token savings required to actually apply
    /// replacements. Below this threshold the call is a no-op.
    pub min_savings_tokens: usize,
}

impl Default for ShakeConfig {
    fn default() -> Self {
        Self {
            protect_window_tokens: 16_384,
            min_elidable_tokens: 400,
            min_savings_tokens: 4_096,
        }
    }
}

/// Result of a single [`shake`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShakeOutcome {
    /// At least one region was elided.
    Shaken {
        /// Number of regions replaced by placeholders.
        regions_elided: usize,
        /// Approximate tokens recovered (using `chars / 4`).
        tokens_saved: usize,
    },
    /// The call did not meet `min_savings_tokens`; `messages` is unchanged.
    NoChange,
}

// ─────────────────────────────────────────────────────────────────────────
// Token estimation
// ─────────────────────────────────────────────────────────────────────────

/// Approximate token count for a UTF-8 string.
///
/// Uses the legacy `chars / 4` heuristic. We divide on `chars().count()`
/// (not byte length) so non-ASCII content is not under-counted.
#[inline]
fn estimate_tokens(text: &str) -> usize {
    text.chars().count() / 4
}

/// Approximate token count for a [`ContentBlock`].
#[inline]
fn estimate_block_tokens(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text(t) => estimate_tokens(&t.text),
        ContentBlock::Thinking(t) => estimate_tokens(&t.thinking),
        ContentBlock::Image(_) => 8,
        ContentBlock::ToolCall(tc) => (tc.name.chars().count() / 4) + 12,
        ContentBlock::Unknown(_) => 10,
    }
}

/// Approximate token count for a [`MessageContent`].
fn estimate_message_content_tokens(content: &MessageContent) -> usize {
    match content {
        MessageContent::Text(s) => estimate_tokens(s),
        MessageContent::Blocks(blocks) => blocks.iter().map(estimate_block_tokens).sum(),
    }
}

/// Approximate token count for a [`Message`].
///
/// Uses [`Message::text_content`] for tool results so the rendered text
/// is the basis; falls back to a structural estimate if rendering fails
/// (which it should not for in-process messages).
fn estimate_message_tokens(message: &Message) -> usize {
    match message {
        Message::User(m) => estimate_message_content_tokens(&m.content),
        Message::Assistant(m) => m.content.iter().map(estimate_block_tokens).sum(),
        Message::ToolResult(m) => match m.text_content() {
            Ok(text) => estimate_tokens(&text),
            Err(_) => m.content.iter().map(estimate_block_tokens).sum(),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Protect boundary
// ─────────────────────────────────────────────────────────────────────────
/// Find the slice index where the protected tail begins (inclusive end).
///
/// Mirrors omp `collectShakeRegions`: a message at index `i` is eligible
/// for shaking when the tokens of messages strictly after it sum to at
/// least `protect_window_tokens` (i.e., a recent tail of that size
/// already exists and is safe to keep). Walking backwards from the end
/// of `messages`, we accumulate tokens AFTER the current index. As soon
/// as that running total crosses the window, the current index is the
/// first eligible message — everything before it is also eligible; the
/// tail `[boundary..]` is protected verbatim.
///
/// Edge cases:
/// - `protect_window_tokens == 0` → boundary = 0 (nothing protected;
///   anything that can be saved is elided).
/// - Empty vector → boundary = 0.
/// - Total tokens below the window → boundary = `messages.len()`
///   (the whole log fits in the protected tail; nothing is eligible).
fn find_protect_boundary(messages: &[Message], protect_window_tokens: usize) -> usize {
    if messages.is_empty() || protect_window_tokens == 0 {
        return 0;
    }
    let mut accumulated_after: usize = 0;
    for (idx, message) in messages.iter().enumerate().rev() {
        if accumulated_after >= protect_window_tokens {
            // The current index is the first one with a sufficiently
            // large tail behind it — start the eligible range here.
            return idx + 1;
        }
        accumulated_after = accumulated_after.saturating_add(estimate_message_tokens(message));
    }
    // Walked the whole log without crossing the window: every index is
    // protected.
    messages.len()
}

// ─────────────────────────────────────────────────────────────────────────
// Candidate collection
// ─────────────────────────────────────────────────────────────────────────

/// A planned elision — applies to a single message.
///
/// We store plain data instead of closures so the candidate type is
/// `Debug`-able and trivially `Clone`-able (the apply phase can then
/// walk candidates in reverse index order without lifetime tangles).
#[derive(Debug, Clone)]
enum Candidate {
    /// Replace the entire content of a `Message::ToolResult`.
    ToolResult {
        /// Index into the message vector.
        index: usize,
        /// Tokens saved by this replacement.
        tokens_saved: usize,
    },
    /// Replace a fenced code block inside a user/assistant message's
    /// text content. We splice the placeholder into the first `Text`
    /// block's `text` field; non-text blocks are untouched.
    CodeBlock {
        /// Index into the message vector.
        index: usize,
        /// Byte offset of the opening fence within the block's text.
        block_start: usize,
        /// Byte offset just past the closing fence.
        block_end: usize,
        /// Pre-rendered placeholder string.
        placeholder: String,
        /// Tokens saved by this replacement.
        tokens_saved: usize,
    },
}

/// Scan messages `[0..boundary)` for elidable regions and compute the
/// total recoverable tokens. Does **not** mutate the input.
fn collect_candidates(
    messages: &[Message],
    boundary: usize,
    min_elidable_tokens: usize,
) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for (index, message) in messages[..boundary].iter().enumerate() {
        match message {
            Message::ToolResult(m) => {
                let original_tokens = match m.text_content() {
                    Ok(text) => estimate_tokens(&text),
                    Err(_) => m.content.iter().map(estimate_block_tokens).sum(),
                };
                if original_tokens >= min_elidable_tokens {
                    let placeholder = format!("[tool result elided (~{original_tokens} tokens)]");
                    let placeholder_tokens = estimate_tokens(&placeholder);
                    let tokens_saved = original_tokens.saturating_sub(placeholder_tokens);
                    candidates.push(Candidate::ToolResult {
                        index,
                        tokens_saved,
                    });
                }
            }
            Message::User(m) => {
                collect_text_candidates(&m.content, index, min_elidable_tokens, &mut candidates);
            }
            Message::Assistant(m) => {
                // Assistant messages hold many block types; we only
                // scan the text within `ContentBlock::Text` blocks.
                // Each text block is scanned independently so non-text
                // blocks (tool calls, images, thinking) survive intact.
                for block in &m.content {
                    if let ContentBlock::Text(t) = block {
                        let content = MessageContent::Text(t.text.clone());
                        collect_text_candidates(
                            &content,
                            index,
                            min_elidable_tokens,
                            &mut candidates,
                        );
                    }
                }
            }
        }
    }
    candidates
}

/// Walk a single `MessageContent`'s text, looking for fenced code blocks
/// large enough to elide.
fn collect_text_candidates(
    content: &MessageContent,
    index: usize,
    min_elidable_tokens: usize,
    out: &mut Vec<Candidate>,
) {
    let Some(text) = content.as_str() else {
        return;
    };
    for_each_elidable_code_block(
        text,
        min_elidable_tokens,
        |block_start, block_end, body, lines| {
            let body_tokens = estimate_tokens(body);
            let placeholder = format!("\n```\n...code block elided ({lines} lines)...\n```\n");
            let placeholder_tokens = estimate_tokens(&placeholder);
            let tokens_saved = body_tokens.saturating_sub(placeholder_tokens);
            if tokens_saved == 0 {
                return;
            }
            out.push(Candidate::CodeBlock {
                index,
                block_start,
                block_end,
                placeholder,
                tokens_saved,
            });
        },
    );
}

/// Invoke `f` once per fenced code block whose body (excluding the
/// fences themselves) has at least `min_elidable_tokens` tokens.
fn for_each_elidable_code_block(
    text: &str,
    min_elidable_tokens: usize,
    mut f: impl FnMut(usize, usize, &str, usize),
) {
    let bytes = text.as_bytes();
    let mut search_from = 0usize;
    while let Some(open_rel) = find_fence_open(bytes, search_from) {
        let open_start = search_from + open_rel;
        let Some(close_rel) = find_fence_close(bytes, open_start + 3) else {
            // Unterminated fence — stop scanning.
            return;
        };
        let close_end = open_start + 3 + close_rel + 3;
        let body_start = open_start + 3;
        let body = &text[body_start..close_end - 3];
        let tokens = estimate_tokens(body);
        if tokens >= min_elidable_tokens {
            let lines = body.lines().count();
            f(open_start, close_end, body, lines);
        }
        search_from = close_end;
    }
}

/// Locate the next opening fence (`` ``` ``) at or after `from`.
///
/// Returns the **byte offset relative to `from`** of the backtick run, or
/// `None` if no opening fence remains.
fn find_fence_open(bytes: &[u8], from: usize) -> Option<usize> {
    if from + 2 >= bytes.len() {
        return None;
    }
    let mut idx = from;
    while idx + 2 < bytes.len() {
        if bytes[idx] == b'`' && bytes[idx + 1] == b'`' && bytes[idx + 2] == b'`' {
            return Some(idx - from);
        }
        idx += 1;
    }
    None
}

/// Locate the closing fence following an opening fence at byte offset
/// `open_start`. `search_from` is the first byte **after** the opening
/// fence's three backticks (so we don't match the opener).
///
/// Returns the **byte offset relative to `search_from`** of the closing
/// backtick run, or `None` if the fence is unterminated.
fn find_fence_close(bytes: &[u8], search_from: usize) -> Option<usize> {
    if search_from + 2 >= bytes.len() {
        return None;
    }
    let mut idx = search_from;
    while idx + 2 < bytes.len() {
        if bytes[idx] == b'`' && bytes[idx + 1] == b'`' && bytes[idx + 2] == b'`' {
            return Some(idx - search_from);
        }
        idx += 1;
    }
    None
}

/// Replace `text[block_start..block_end]` with `placeholder`, returning
/// the resulting `String`.
fn replace_code_block(
    text: &str,
    block_start: usize,
    block_end: usize,
    placeholder: &str,
) -> String {
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..block_start]);
    out.push_str(placeholder);
    out.push_str(&text[block_end..]);
    out
}

// ─────────────────────────────────────────────────────────────────────────
// Application
// ─────────────────────────────────────────────────────────────────────────

/// Apply every planned candidate to `messages`.
///
/// Candidates are applied in reverse index order so earlier replacements
/// don't invalidate later indices.
fn apply_candidates(messages: &mut [Message], mut candidates: Vec<Candidate>) {
    // Sort by descending index so we mutate back-to-front and indices
    // for earlier entries stay valid as we go.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.index()));

    for candidate in candidates {
        apply_one(messages, candidate);
    }
}

impl Candidate {
    fn index(&self) -> usize {
        match self {
            Candidate::ToolResult { index, .. } => *index,
            Candidate::CodeBlock { index, .. } => *index,
        }
    }
}

fn apply_one(messages: &mut [Message], candidate: Candidate) {
    match candidate {
        Candidate::ToolResult { index, .. } => {
            let Some(Message::ToolResult(tr)) = messages.get_mut(index) else {
                return;
            };
            let original_tokens = match tr.text_content() {
                Ok(text) => estimate_tokens(&text),
                Err(_) => tr.content.iter().map(estimate_block_tokens).sum(),
            };
            let placeholder = format!("[tool result elided (~{original_tokens} tokens)]");
            tr.content = vec![ContentBlock::Text(TextContent::new(placeholder))];
        }
        Candidate::CodeBlock {
            index,
            block_start,
            block_end,
            placeholder,
            ..
        } => {
            let Some(message) = messages.get_mut(index) else {
                return;
            };
            match message {
                Message::User(m) => {
                    rewrite_message_content(&mut m.content, block_start, block_end, &placeholder)
                }
                Message::Assistant(m) => {
                    // Replace the first text block whose `text` field
                    // is at least `block_end` chars long. This handles
                    // the simple case where the code block lives in
                    // one text block; multi-block assistant messages
                    // with the same code block split across blocks are
                    // not supported (and are extremely rare in
                    // practice).
                    for block in &mut m.content {
                        if let ContentBlock::Text(t) = block
                            && t.text.len() >= block_end
                        {
                            t.text =
                                replace_code_block(&t.text, block_start, block_end, &placeholder);
                            return;
                        }
                    }
                }
                Message::ToolResult(_) => {
                    // Tool result content is rewritten by the dedicated
                    // candidate variant; this branch is unreachable.
                }
            }
        }
    }
}

/// Splice a placeholder into a `MessageContent`. Operates on the first
/// text payload available — `MessageContent::Text` directly, or the
/// first `ContentBlock::Text` inside `MessageContent::Blocks`.
fn rewrite_message_content(
    content: &mut MessageContent,
    block_start: usize,
    block_end: usize,
    placeholder: &str,
) {
    match content {
        MessageContent::Text(s) => {
            if s.len() >= block_end {
                *s = replace_code_block(s, block_start, block_end, placeholder);
            }
        }
        MessageContent::Blocks(blocks) => {
            for block in blocks {
                if let ContentBlock::Text(t) = block
                    && t.text.len() >= block_end
                {
                    t.text = replace_code_block(&t.text, block_start, block_end, placeholder);
                    return;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────────

/// Shake the message log: elide large tool results and code blocks from
/// the older portion of `messages`, leaving the recent
/// [`protect_window_tokens`](ShakeConfig::protect_window_tokens) intact.
///
/// If aggregate savings are below
/// [`min_savings_tokens`](ShakeConfig::min_savings_tokens) the call is a
/// no-op: `messages` is unchanged and the outcome is [`ShakeOutcome::NoChange`].
/// Otherwise every eligible region is replaced in a single pass.
// The signature takes `&mut Vec<Message>` to match the spec; callers pass
// a `Vec<Message>` from the agent log, and slice deref coercion is
// available internally. The clippy lint is silenced locally rather than
// file-wide to keep the rest of the file slice-clean.
#[allow(clippy::ptr_arg)]
pub fn shake(messages: &mut Vec<Message>, config: &ShakeConfig) -> ShakeOutcome {
    let boundary = find_protect_boundary(messages, config.protect_window_tokens);
    if boundary == 0 {
        // Either the log is empty, the protect window covers
        // everything, or the budget is zero — nothing is eligible.
        return ShakeOutcome::NoChange;
    }

    let candidates = collect_candidates(messages, boundary, config.min_elidable_tokens);
    if candidates.is_empty() {
        return ShakeOutcome::NoChange;
    }

    let total_savings: usize = candidates.iter().map(Candidate::tokens_saved).sum();

    if total_savings < config.min_savings_tokens {
        return ShakeOutcome::NoChange;
    }

    let regions_elided = candidates.len();
    apply_candidates(messages, candidates);
    ShakeOutcome::Shaken {
        regions_elided,
        tokens_saved: total_savings,
    }
}

impl Candidate {
    fn tokens_saved(&self) -> usize {
        match self {
            Candidate::ToolResult { tokens_saved, .. } => *tokens_saved,
            Candidate::CodeBlock { tokens_saved, .. } => *tokens_saved,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oxicode_ai::{Api, AssistantMessage, ToolResultMessage, UserMessage};

    /// Build a small user message.
    fn user_msg(text: &str) -> Message {
        Message::User(UserMessage::new(text.to_string()))
    }

    #[allow(dead_code)]
    fn assistant_msg(text: &str) -> Message {
        let mut msg = AssistantMessage::new(Api::AnthropicMessages, "mock", "test-model");
        msg.content.push(ContentBlock::Text(TextContent::new(text)));
        Message::Assistant(msg)
    }

    /// Build a tool result message whose text payload is `text`.
    fn tool_result_msg(tool_call_id: &str, tool_name: &str, text: &str) -> Message {
        Message::ToolResult(ToolResultMessage::new(
            tool_call_id.to_string(),
            tool_name.to_string(),
            vec![ContentBlock::Text(TextContent::new(text.to_string()))],
        ))
    }

    /// Build a string of `n` ASCII chars.
    fn chars(n: usize) -> String {
        "a".repeat(n)
    }

    /// Compact `ShakeConfig` used by the unit tests. `protect_window_tokens`
    /// is small enough that a single trailing message can exceed it, and
    /// the elision/savings thresholds are scaled to match.
    const CFG: ShakeConfig = ShakeConfig {
        protect_window_tokens: 100,
        min_elidable_tokens: 50,
        min_savings_tokens: 200,
    };

    #[test]
    fn test_shake_elides_large_tool_result() {
        // 8 000 chars ≈ 2 000 tokens — clears `min_elidable_tokens` and
        // yields savings that clear `min_savings_tokens`. The trailing
        // user msg (~125 tokens) crosses the 100-token protect window,
        // so the boundary lands past the tool result and index 0 is
        // eligible.
        let mut messages = vec![
            tool_result_msg("call-1", "search", &chars(8_000)),
            user_msg(&chars(500)),
        ];
        let outcome = shake(&mut messages, &CFG);
        match outcome {
            ShakeOutcome::Shaken {
                regions_elided,
                tokens_saved,
            } => {
                assert_eq!(regions_elided, 1);
                assert!(tokens_saved >= 200);
            }
            other => panic!("expected Shaken, got {other:?}"),
        }
        match &messages[0] {
            Message::ToolResult(tr) => {
                assert_eq!(tr.content.len(), 1);
                let rendered = tr.text_content().expect("renderable");
                assert!(
                    rendered.contains("tool result elided"),
                    "unexpected tool result text: {rendered:?}"
                );
            }
            other => panic!("expected ToolResult variant, got {other:?}"),
        }
    }

    #[test]
    fn test_shake_preserves_protect_window() {
        // 4 tool results, each 8 000 chars ≈ 2 000 tokens. With
        // `protect_window_tokens = 2 500`, walking back from the end we
        // accumulate: 0 → 2 000 → 4 000. The 4 000-token cumulative
        // (i.e. the two most-recent messages) crosses 2 500, so the
        // boundary lands at index 2 — the first two are eligible; the
        // last two are inside the protected tail.
        let mut messages = vec![
            tool_result_msg("call-1", "search", &chars(8_000)),
            tool_result_msg("call-2", "search", &chars(8_000)),
            tool_result_msg("call-3", "search", &chars(8_000)),
            tool_result_msg("call-4", "search", &chars(8_000)),
        ];
        let snapshot_before: Vec<String> = messages
            .iter()
            .map(|m| match m {
                Message::ToolResult(tr) => tr.text_content().unwrap_or_default(),
                _ => String::new(),
            })
            .collect();

        let cfg = ShakeConfig {
            protect_window_tokens: 2_500,
            ..CFG
        };
        let outcome = shake(&mut messages, &cfg);
        assert!(matches!(outcome, ShakeOutcome::Shaken { .. }));

        let len = messages.len();
        // The two most-recent tool results must be untouched.
        for (idx, original) in snapshot_before.iter().enumerate().rev().take(2) {
            let preserved = match &messages[idx] {
                Message::ToolResult(tr) => tr.text_content().unwrap_or_default(),
                _ => panic!("expected ToolResult at index {idx}"),
            };
            assert_eq!(
                &preserved, original,
                "tool result at index {idx} was mutated but should be inside the protect window"
            );
        }
        // The first two must now contain the placeholder.
        for (msg, idx) in messages.iter().take(len - 2).zip(0..) {
            let rendered = match msg {
                Message::ToolResult(tr) => tr.text_content().unwrap_or_default(),
                _ => panic!("expected ToolResult at index {idx}"),
            };
            assert!(
                rendered.contains("tool result elided"),
                "tool result at index {idx} was not elided: {rendered:?}"
            );
        }
    }

    #[test]
    fn test_shake_no_change_when_insufficient_savings() {
        // 3 tool results, each 200 chars ≈ 50 tokens — they ARE
        // eligible candidates (≥ `min_eligible_tokens`) but `min_savings_tokens`
        // is bumped to 5 000 so aggregate savings (~3 × 45 = 135)
        // cannot meet it; outcome must be NoChange and the messages
        // must be untouched.
        let mut messages = vec![
            tool_result_msg("call-1", "echo", &chars(200)),
            tool_result_msg("call-2", "echo", &chars(200)),
            tool_result_msg("call-3", "echo", &chars(200)),
        ];
        let snapshot_before: Vec<String> = messages
            .iter()
            .map(|m| match m {
                Message::ToolResult(tr) => tr.text_content().unwrap_or_default(),
                _ => String::new(),
            })
            .collect();

        let cfg = ShakeConfig {
            protect_window_tokens: 50,
            min_elidable_tokens: 40,
            min_savings_tokens: 5_000,
        };
        let outcome = shake(&mut messages, &cfg);
        assert_eq!(outcome, ShakeOutcome::NoChange);

        let snapshot_after: Vec<String> = messages
            .iter()
            .map(|m| match m {
                Message::ToolResult(tr) => tr.text_content().unwrap_or_default(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(snapshot_before, snapshot_after);
    }

    #[test]
    fn test_shake_elides_large_code_block() {
        // 8 000-char fenced code body ≈ 2 000 tokens — clears
        // `min_elidable_tokens`. The trailing user msg (~125 tokens)
        // crosses the 100-token protect window, so the boundary sits
        // past index 0 and the code block is found.
        let code_body = chars(8_000);
        let user_text = format!("x\n```rust\n{code_body}\n```\n");
        let mut messages = vec![user_msg(&user_text), user_msg(&chars(500))];
        let outcome = shake(&mut messages, &CFG);
        match outcome {
            ShakeOutcome::Shaken {
                regions_elided,
                tokens_saved,
            } => {
                assert_eq!(regions_elided, 1);
                assert!(tokens_saved >= 200);
            }
            other => panic!("expected Shaken, got {other:?}"),
        }
        let rendered = messages[0].text_content().expect("renderable");
        assert!(
            rendered.contains("code block elided"),
            "code block was not replaced; got: {rendered:?}"
        );
        assert!(
            !rendered.contains(&code_body),
            "original code body should have been replaced"
        );
    }
}
