//! `SnapcompactCompactor` — `oxi-ai::Compactor` implementation backed by
//! the snapcompact PNG renderer in `oxi-snapcompact`.
//!
//! Lives in the SDK layer (not `oxi-ai`) per the foundation-layer
//! invariant — `oxi-ai` defines the `Compactor` trait and the
//! `CompactionStrategy::Snapcompact` variant, but holds no dependency
//! on `oxi-snapcompact`. The concrete impl is here because the SDK
//! is the "contract + reference impl" layer that already depends on
//! both `oxi-ai` and `oxi-snapcompact`.
//!
//! See `docs/designs/2026-07-18-stub-completion.md` §4.5.

use std::pin::Pin;
use std::sync::Arc;

use oxi_ai::Message;
use oxi_ai::compaction::{CompactedContext, CompactionError, CompactionMetadata, Compactor};
use oxi_snapcompact::Shape;

/// Bitmap-frame compactor that renders the discarded tail of a
/// conversation as PNG frames via the snapcompact pipeline.
///
/// Unlike the LLM compactor, this compactor makes no LLM call — the
/// local renderer is the only cost. Frames are addressed at
/// vision-capable models (Anthropic, OpenAI, Google) that read the
/// PNGs back directly.
///
/// ## Limits
///
/// Snapcompact always returns **real PNG bytes** for each frame or
/// surfaces the failure via [`CompactionError::LlmError`]. There is no
/// "no-op renderer" fallback: a rendering failure becomes a
/// compaction failure (intentional, so the operator notices).
pub struct SnapcompactCompactor {
    /// Shape used to rasterize the discarded text. None → snapcompact
    /// resolves a model-aware default per `Model::id`.
    shape: Option<Shape>,
    /// Per-frame source-text character limit. Frames longer than this
    /// are chunked; shorter frames are still rendered (snapcompact
    /// is happy with empty-ish frames).
    frame_chars: usize,
    /// Maximum frames per compaction.
    max_frames: u32,
}

impl Default for SnapcompactCompactor {
    fn default() -> Self {
        Self {
            shape: None,
            frame_chars: 4000,
            max_frames: 8,
        }
    }
}

impl SnapcompactCompactor {
    /// Construct with snapcompact's model-aware shape resolution.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin a specific shape (overrides model-aware resolution).
    pub fn with_shape(mut self, shape: Shape) -> Self {
        self.shape = Some(shape);
        self
    }

    /// Set the per-frame source-text character limit.
    pub fn with_frame_chars(mut self, frame_chars: usize) -> Self {
        self.frame_chars = frame_chars.max(64);
        self
    }

    /// Set the maximum number of frames per compaction.
    pub fn with_max_frames(mut self, max_frames: u32) -> Self {
        self.max_frames = max_frames;
        self
    }

    /// Borrow the configured shape (if pinned).
    pub fn shape(&self) -> Option<&Shape> {
        self.shape.as_ref()
    }
}

impl Compactor for SnapcompactCompactor {
    fn estimate_tokens(&self, messages: &[Message]) -> usize {
        // Default text-based estimator (matches LlmCompactor's
        // heuristic: `bytes / 4`). Snapcompact's true cost is
        // image-charged, not text-charged, but consumers that go
        // through the `dyn Compactor` surface want a single
        // accounting path.
        messages
            .iter()
            .map(|m| m.text_content().map(|t| t.len() / 4).unwrap_or(0))
            .sum()
    }

    fn compact<'a>(
        &'a self,
        messages: &'a [Message],
        instruction: Option<&'a str>,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<CompactedContext, CompactionError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            if messages.is_empty() {
                return Err(CompactionError::NoMessagesToCompact);
            }

            // Build the serializable text envelope from the messages.
            // `serialize_conversation` from oxi-snapcompact handles the
            // role prefix + tool-output truncation; we then chunk into
            // frames of `frame_chars` characters and render each frame
            // through the PNG renderer.
            let mut text = String::new();
            if let Some(instr) = instruction {
                text.push_str(&format!("[instruction] {instr}\n\n"));
            }
            text.push_str(&oxi_snapcompact::serialize_conversation(
                &flatten_messages(messages),
                self.frame_chars,
            ));

            let frame_count = max_frame_count(text.chars().count(), self.frame_chars);
            let frames_to_render = frame_count.min(self.max_frames as usize).max(1);

            let original_tokens = self.estimate_tokens(messages);

            // Render each chunk into a PNG frame via oxi-snapcompact.
            let mut frames: Vec<(u32, Vec<u8>)> = Vec::with_capacity(frames_to_render);
            let mut idx: u32 = 0;
            'outer: for chunk in chunks(&text, self.frame_chars).take(frames_to_render) {
                let prep = oxi_snapcompact::prepare(&chunk, 200_000, self.frame_chars);
                let opts = oxi_snapcompact::CompactOptions {
                    shape: self.shape.clone(),
                    model_id: String::new(),
                    max_frames: 1,
                };
                let result = oxi_snapcompact::compact(&prep, &opts);
                if result.frames.is_empty() {
                    return Err(CompactionError::LlmError(format!(
                        "snapcompact produced no frames for chunk {idx}"
                    )));
                }
                for f in result.frames {
                    if f.bytes.is_empty() {
                        return Err(CompactionError::LlmError(format!(
                            "snapcompact frame {} returned empty bytes (render failure)",
                            f.index
                        )));
                    }
                    frames.push((idx, f.bytes));
                    idx += 1;
                    if idx as usize == self.max_frames as usize {
                        break 'outer;
                    }
                }
            }

            // No message is "kept" in pure snapcompact mode — the
            // rendered PNGs are the compacted context. The caller
            // (agent loop) is expected to attach the PNGs as image
            // content to a single assistant message.
            let metadata = CompactionMetadata::new(
                original_tokens,
                estimate_frame_tokens(frames.len()),
                frames.len(),
                0,
                0.0, // ratio is N/A for bitmap compaction
            );

            let mut context = CompactedContext::new(
                format!("[snapcompact] {} frames", frames.len()),
                Vec::new(),
                frames.len(),
                metadata,
            );
            context.frames = Some(Arc::new(frames));
            Ok(context)
        })
    }
}

// ── helpers ────────────────────────────────────────────────────

/// Heuristic: a PNG frame ≈ 1500 tokens (vision models typically
/// charge per image, not per pixel; this is a conservative
/// overestimate that errs on the side of "need to compact more").
fn estimate_frame_tokens(frames: usize) -> usize {
    frames.saturating_mul(1500)
}

fn max_frame_count(chars: usize, frame_chars: usize) -> usize {
    if frame_chars == 0 {
        return 0;
    }
    chars.div_ceil(frame_chars)
}

fn chunks(text: &str, frame_chars: usize) -> impl Iterator<Item = String> + '_ {
    let mut remaining = text;
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        // Split on char boundary to avoid mid-codepoint cuts.
        let n = remaining.chars().count().min(frame_chars);
        let cut = remaining
            .char_indices()
            .nth(n)
            .map(|(idx, _)| idx)
            .unwrap_or(remaining.len());
        let (head, tail) = remaining.split_at(cut);
        remaining = tail;
        Some(head.to_string())
    })
}

/// Flatten a slice of [`Message`] into a single normalized text
/// string suitable for `serialize_conversation`. Each message is
/// prefixed with its role.
fn flatten_messages(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m {
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "tool",
        };
        let text = m.text_content().unwrap_or_default();
        if !text.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(role);
            out.push_str(": ");
            out.push_str(&text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let c = SnapcompactCompactor::new();
        assert!(c.shape().is_none());
        assert_eq!(c.frame_chars, 4000);
        assert_eq!(c.max_frames, 8);
    }

    #[test]
    fn shape_pinning_works() {
        let shape = oxi_snapcompact::SHAPES[0].clone();
        let c = SnapcompactCompactor::new().with_shape(shape.clone());
        assert!(c.shape().is_some());
        assert_eq!(c.shape().unwrap().name, shape.name);
    }

    #[test]
    fn frame_chars_floor() {
        let c = SnapcompactCompactor::new().with_frame_chars(0);
        // 0 is floored to 64 (avoid divide-by-zero / tiny frames).
        assert_eq!(c.frame_chars, 64);
    }

    #[test]
    fn empty_messages_returns_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let compactor = SnapcompactCompactor::new();
        let err = rt
            .block_on(async { compactor.compact(&[], None).await })
            .unwrap_err();
        assert!(matches!(err, CompactionError::NoMessagesToCompact));
    }

    #[test]
    fn compact_real_messages_produces_png_frames() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let messages = vec![Message::user(
            "Hello world. This is a test message that should be rendered as a PNG frame by snapcompact.",
        )];
        let compactor = SnapcompactCompactor::new();
        let result = rt
            .block_on(async { compactor.compact(&messages, None).await })
            .expect("compaction should succeed");
        let frames = result.frames.as_ref().expect("frames should be attached");
        assert!(!frames.is_empty(), "should produce ≥1 frame");
        for (idx, bytes) in frames.iter() {
            assert!(!bytes.is_empty(), "frame {idx} must be non-empty");
            // PNG magic header.
            assert_eq!(
                &bytes[..8],
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
                "frame {idx} must be PNG"
            );
        }
    }
}
