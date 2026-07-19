//! Snapcompact — bitmap-frame context compression for vision-capable LLMs.
//!
//! Ported from [omp](https://github.com/can1357/oh-my-pi) (MIT) —
//! `packages/snapcompact/src/snapcompact.ts`. See
//! `docs/ref-porter/xai-org-grok-build.md` section E2 for design rationale.
//!
//! ## How it works
//!
//! Instead of asking an LLM to summarize discarded history, snapcompact
//! renders discarded text as dense bitmap frames that vision-capable
//! models read back directly. Local and deterministic — no LLM call,
//! no API key, no latency beyond rendering.
//!
//! ## Scope of this crate
//!
//! Shape system, conversation serialization, shape selection, and
//! the full text→PNG rasterizer (ported from omp's
//! `pi-natives/src/snapcompact.rs` — see the [`renderer`] submodule).
//! The renderer uses bundled BDF/HEX bitmap fonts plus the bundled
//! Silver TrueType font for non-Latin glyphs, all under permissive
//! licenses (see `NOTICE.md`).
//!
//! [`compact()`] always returns real PNG-encoded bytes — there is no
//! "no-op renderer" path. When a frame fails to render, the error is
//! logged via `tracing` and that frame's bytes are empty; the caller
//! can decide whether to drop or surface partial results.
use serde::{Deserialize, Serialize};
pub mod renderer;

// ── Shape system ──────────────────────────────────────────────────────

/// One eval-validated frame shape.
///
/// `name` is a stable identifier matching omp's `SHAPE_VARIANTS` table.
/// The MVP carries a hard-coded name on each shape entry; future
/// extension can compute names from fields, but the table is the source
/// of truth for now (matches omp SHAPE_VARIANTS exactly).
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Stable name (e.g. "11on16-bw"). Used as lookup key.
    pub name: &'static str,
    /// Bundled font.
    pub font: Font,
    pub cell_width: u32,
    pub cell_height: u32,
    pub stretch: bool,
    pub variant: Variant,
    pub stopword_dim: bool,
    pub columns: u32,
    pub line_repeat: u32,
    pub frame_size: u32,
}

/// Bundled font name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Font {
    FiveByEight,
    EightByEight,
    SixByTwelve,
    EightByThirteen,
    Silver,
}

impl Font {
    /// String identifier matching omp's shape table.
    pub fn as_str(&self) -> &'static str {
        match self {
            Font::FiveByEight => "5x8",
            Font::EightByEight => "8x8",
            Font::SixByTwelve => "6x12",
            Font::EightByThirteen => "8x13",
            Font::Silver => "silver",
        }
    }
}

/// Ink variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Cycle six hues at sentence boundaries.
    Sent,
    /// Plain black ink — best for Anthropic vision readers.
    Bw,
}

impl Variant {
    pub fn as_str(&self) -> &'static str {
        match self {
            Variant::Sent => "sent",
            Variant::Bw => "bw",
        }
    }
}

/// Eval-validated shape table. Names match omp's `SHAPE_VARIANTS` keys
/// exactly so the Rust and TS registries stay in sync.
pub const SHAPES: &[Shape] = &[
    // Redundancy-coded double-print (line_repeat=2).
    Shape {
        name: "8x8r-bw",
        font: Font::EightByEight,
        cell_width: 8,
        cell_height: 8,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 2,
        frame_size: 1568,
    },
    Shape {
        name: "8x8r-sent",
        font: Font::EightByEight,
        cell_width: 8,
        cell_height: 8,
        stretch: false,
        variant: Variant::Sent,
        stopword_dim: false,
        columns: 1,
        line_repeat: 2,
        frame_size: 1568,
    },
    // Standard 8x8 dense shapes. Note: `u` suffix is part of the name
    // even though cell==natural; it flags the variant family.
    Shape {
        name: "8x8u-bw",
        font: Font::EightByEight,
        cell_width: 8,
        cell_height: 8,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    Shape {
        name: "8x8u-sent",
        font: Font::EightByEight,
        cell_width: 8,
        cell_height: 8,
        stretch: false,
        variant: Variant::Sent,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    // Squeezed 8x8 (Lanczos-scaled to 6x6 cell).
    Shape {
        name: "6x6u-bw",
        font: Font::EightByEight,
        cell_width: 6,
        cell_height: 6,
        stretch: true,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    Shape {
        name: "6x6u-sent",
        font: Font::EightByEight,
        cell_width: 6,
        cell_height: 6,
        stretch: true,
        variant: Variant::Sent,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    // 5x8 at 2576px frame edge.
    Shape {
        name: "5x8-bw",
        font: Font::FiveByEight,
        cell_width: 5,
        cell_height: 8,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 2576,
    },
    Shape {
        name: "5x8-sent",
        font: Font::FiveByEight,
        cell_width: 5,
        cell_height: 8,
        stretch: false,
        variant: Variant::Sent,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 2576,
    },
    // 6x12-dim: dim stopwords in gray ink.
    Shape {
        name: "6x12-dim",
        font: Font::SixByTwelve,
        cell_width: 6,
        cell_height: 12,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: true,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    // 8x13 natural-cell shape.
    Shape {
        name: "8x13-bw",
        font: Font::EightByThirteen,
        cell_width: 8,
        cell_height: 13,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    // 8x13 on extra-leading pitches.
    Shape {
        name: "8on16-bw",
        font: Font::EightByThirteen,
        cell_width: 8,
        cell_height: 16,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    Shape {
        name: "8on22-bw",
        font: Font::EightByThirteen,
        cell_width: 8,
        cell_height: 22,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    // 11on16-bw: extra tracking (11px advance) on 8x13.
    Shape {
        name: "11on16-bw",
        font: Font::EightByThirteen,
        cell_width: 11,
        cell_height: 16,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    // Silver TTF (16x16 grid) — CJK.
    Shape {
        name: "silver16-bw",
        font: Font::Silver,
        cell_width: 16,
        cell_height: 16,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 1,
        line_repeat: 1,
        frame_size: 1568,
    },
    // Newspaper column layouts (doc shapes).
    Shape {
        name: "doc-8on16-bw",
        font: Font::EightByThirteen,
        cell_width: 8,
        cell_height: 16,
        stretch: false,
        variant: Variant::Bw,
        stopword_dim: false,
        columns: 2,
        line_repeat: 1,
        frame_size: 1568,
    },
    Shape {
        name: "doc-8on16-sent",
        font: Font::EightByThirteen,
        cell_width: 8,
        cell_height: 16,
        stretch: false,
        variant: Variant::Sent,
        stopword_dim: false,
        columns: 2,
        line_repeat: 1,
        frame_size: 1568,
    },
    Shape {
        name: "doc-8on16-sent-dim",
        font: Font::EightByThirteen,
        cell_width: 8,
        cell_height: 16,
        stretch: false,
        variant: Variant::Sent,
        stopword_dim: true,
        columns: 2,
        line_repeat: 1,
        frame_size: 1568,
    },
];

impl Shape {
    /// The stable name (matches omp SHAPE_VARIANTS key).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Approximate characters per frame.
    pub fn chars_per_frame(&self) -> u32 {
        self.frame_size / self.cell_width.max(1)
    }

    /// Approximate rows per frame.
    pub fn rows_per_frame(&self) -> u32 {
        self.frame_size / self.cell_height.max(1)
    }
}

/// Pick the eval-validated shape for `model_id`.
pub fn resolve_shape(model_id: &str) -> Shape {
    let id = model_id.to_ascii_lowercase();
    if id.contains("claude") || id.contains("anthropic") {
        return lookup_owned("11on16-bw").unwrap_or_else(|| SHAPES[0].clone());
    }
    if id.contains("gpt-5") || id.contains("gpt-4.1") || id.contains("o3") || id.contains("o4") {
        return lookup_owned("8on22-bw").unwrap_or_else(|| SHAPES[0].clone());
    }
    if id.contains("gemini") {
        return lookup_owned("8on22-bw").unwrap_or_else(|| SHAPES[0].clone());
    }
    // Unknown provider: Anthropic shape (safest default for vision).
    lookup_owned("11on16-bw").unwrap_or_else(|| SHAPES[0].clone())
}

fn lookup_owned(name: &str) -> Option<Shape> {
    SHAPES.iter().find(|s| s.name == name).cloned()
}

// ── Text normalization ────────────────────────────────────────────────

/// Normalize conversation text for rasterization.
pub fn normalize(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            i += 1;
            continue;
        }
        let c = input[i..].chars().next().unwrap_or(' ');
        match c {
            '─' | '━' | '│' | '┃' | '┌' | '┍' | '┎' | '┏' | '┐' | '┑' | '┒' | '┓' | '└' | '┕'
            | '┖' | '┗' | '┘' | '┙' | '┚' | '┛' | '├' | '┝' | '┞' | '┟' | '┠' | '┡' | '┢' | '┣'
            | '┤' | '┥' | '┦' | '┧' | '┨' | '┩' | '┪' | '┫' | '┬' | '┭' | '┮' | '┯' | '┰' | '┱'
            | '┲' | '┳' | '┴' | '┵' | '┶' | '┷' | '┸' | '┹' | '┺' | '┻' | '┼' | '┽' | '┾' | '┿' =>
            {
                out.push('-');
                i += c.len_utf8();
            }
            '═' => {
                out.push('=');
                i += c.len_utf8();
            }
            '║' => {
                out.push('|');
                i += c.len_utf8();
            }
            '\n' => {
                out.push('\u{2588}');
                i += 1;
                while i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
            }
            ' ' | '\t' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                i += c.len_utf8();
            }
            _ => {
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

// ── Preparation ──────────────────────────────────────────────────────

/// Inputs to a compaction pass.
#[derive(Debug, Clone)]
pub struct CompactPreparation {
    pub text: String,
    pub bounded_text: String,
    pub remaining_text: String,
}

/// Render the conversation text into a serializable compact envelope.
pub fn prepare(input: &str, tool_result_max_chars: usize, text_limit: usize) -> CompactPreparation {
    let text = serialize_conversation(input, tool_result_max_chars);
    let bounded = bounded_slice(&text, text_limit);
    let consumed = bounded.chars().count();
    let remaining = if consumed >= text.chars().count() {
        String::new()
    } else {
        text.chars().skip(consumed).collect()
    };
    CompactPreparation {
        text,
        bounded_text: bounded,
        remaining_text: remaining,
    }
}

/// Compact conversation text to one line per turn.
pub fn serialize_conversation(input: &str, tool_result_max_chars: usize) -> String {
    let mut out = String::new();
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line.starts_with("Tool:") {
            let truncated = truncate_tool_output(trimmed, tool_result_max_chars);
            out.push_str(&truncated);
            out.push('\n');
        } else {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

fn truncate_tool_output(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let head_ratio = 0.6;
    let head = (max_chars as f64 * head_ratio) as usize;
    let tail = max_chars.saturating_sub(head);
    let chars: Vec<char> = line.chars().collect();
    let head_str: String = chars.iter().take(head).collect();
    let tail_str: String = chars
        .iter()
        .skip(chars.len().saturating_sub(tail))
        .collect();
    format!("{head_str}…[truncated]…{tail_str}")
}

fn bounded_slice(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

// ── Compaction envelope ──────────────────────────────────────────────

/// Options for [`compact`].
#[derive(Debug, Clone)]
pub struct CompactOptions {
    pub model_id: String,
    pub max_frames: u32,
    pub shape: Option<Shape>,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            model_id: String::new(),
            max_frames: 80,
            shape: None,
        }
    }
}

/// Outcome of a compaction pass.
#[derive(Debug, Clone)]
pub struct CompactResult {
    pub summary: String,
    pub source_text: String,
    pub frames: Vec<FrameRef>,
    pub shape: Shape,
}

/// A reference to a rendered frame.
#[derive(Debug, Clone)]
pub struct FrameRef {
    pub index: u32,
    pub source_start: usize,
    pub source_end: usize,
    pub bytes: Vec<u8>,
}

/// The minimal envelope returned by [`compact`] when no renderer is
/// available — shape chosen, text chunked into per-frame source ranges,
/// but frame bytes are empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundedSource {
    pub lead_in: String,
    pub text: String,
    pub frames: Vec<FrameRange>,
}

/// Per-frame source range.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameRange {
    pub index: u32,
    pub source_start: usize,
    pub source_end: usize,
}

impl CompactResult {
    /// Drop the frame bytes; the bounded source + ranges remain.
    pub fn into_bounded_source(&self) -> BoundedSource {
        BoundedSource {
            lead_in: self.summary.clone(),
            text: self.source_text.clone(),
            frames: self
                .frames
                .iter()
                .map(|f| FrameRange {
                    index: f.index,
                    source_start: f.source_start,
                    source_end: f.source_end,
                })
                .collect(),
        }
    }
}

/// Convert a [`Shape`] into the renderer's option struct.
///
/// The shape's `font` / `cell_width` / `cell_height` / `variant` /
/// `line_repeat` / `stretch` / `columns` fields map directly onto
/// [`crate::renderer::SnapcompactRenderOptions`].
fn shape_to_render_options(shape: &Shape) -> crate::renderer::SnapcompactRenderOptions {
    crate::renderer::SnapcompactRenderOptions {
        size: shape.frame_size,
        font: Some(shape.font.as_str().to_string()),
        cell_width: Some(shape.cell_width),
        cell_height: Some(shape.cell_height),
        variant: Some(shape.variant.as_str().to_string()),
        line_repeat: Some(shape.line_repeat),
        stretch: Some(shape.stretch),
        columns: Some(shape.columns),
    }
}

/// Run a compaction pass — render each frame slice through the
/// pi-natives ported renderer (`render_snapcompact_png`) so the
/// returned bytes are real PNG-encoded image data.
///
/// Errors propagate as `Vec::new()` for the offending frame (so the
/// caller still gets partial results) and the failure is logged via
/// `tracing`. This is the only [`compact`] surface — the old
/// `NoopRenderer` / `FrameRenderer` / `compact_with` indirection has
/// been removed: there is one renderer, and it always returns real
/// bytes.
pub fn compact(prep: &CompactPreparation, options: &CompactOptions) -> CompactResult {
    let shape: Shape = options
        .shape
        .clone()
        .unwrap_or_else(|| resolve_shape(&options.model_id));
    let chars_per_frame = shape.chars_per_frame() as usize;
    let frames = chunk_into_frames(
        &prep.bounded_text,
        chars_per_frame,
        options.max_frames as usize,
    );
    let render_opts = shape_to_render_options(&shape);
    let rendered: Vec<FrameRef> = frames
        .iter()
        .map(|range| {
            let slice = slice_char_range(&prep.bounded_text, range.source_start, range.source_end);
            let bytes = match crate::renderer::render_snapcompact_png(slice, render_opts.clone()) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        frame = range.index,
                        shape = %shape.name,
                        error = %e,
                        "snapcompact render failed; emitting empty frame"
                    );
                    Vec::new()
                }
            };
            FrameRef {
                index: range.index,
                source_start: range.source_start,
                source_end: range.source_end,
                bytes,
            }
        })
        .collect();
    CompactResult {
        summary: render_lead_in(&prep.text),
        source_text: prep.bounded_text.clone(),
        frames: rendered,
        shape,
    }
}

fn chunk_into_frames(text: &str, chars_per_frame: usize, max_frames: usize) -> Vec<FrameRange> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    if chars_per_frame == 0 || chars.is_empty() || max_frames == 0 {
        return out;
    }
    let mut start = 0usize;
    let mut idx: u32 = 0;
    while start < chars.len() && idx < max_frames as u32 {
        let end = (start + chars_per_frame).min(chars.len());
        out.push(FrameRange {
            index: idx,
            source_start: start,
            source_end: end,
        });
        start = end;
        idx += 1;
    }
    out
}

fn slice_char_range(s: &str, start: usize, end: usize) -> String {
    s.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn render_lead_in(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let head: String = chars.iter().take(120).collect();
    format!("Resume prior conversation. {head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_table_includes_all_omp_variants() {
        for name in &[
            "8x8r-bw",
            "8x8r-sent",
            "8x8u-bw",
            "8x8u-sent",
            "6x6u-bw",
            "6x6u-sent",
            "5x8-bw",
            "5x8-sent",
            "6x12-dim",
            "8x13-bw",
            "8on16-bw",
            "8on22-bw",
            "11on16-bw",
            "silver16-bw",
            "doc-8on16-bw",
            "doc-8on16-sent",
            "doc-8on16-sent-dim",
        ] {
            assert!(
                lookup_owned(name).is_some(),
                "missing shape `{name}` from registry"
            );
        }
    }

    #[test]
    fn shape_name_round_trip() {
        for s in SHAPES {
            assert_eq!(lookup_owned(s.name).map(|x| x.name), Some(s.name));
        }
    }

    #[test]
    fn resolve_shape_anthropic_picks_11on16() {
        let s = resolve_shape("claude-3-5-sonnet-20241022");
        assert_eq!(s.name, "11on16-bw");
    }

    #[test]
    fn resolve_shape_openai_picks_8on22() {
        let s = resolve_shape("gpt-5.5");
        assert_eq!(s.name, "8on22-bw");
    }

    #[test]
    fn resolve_shape_google_picks_8on22() {
        let s = resolve_shape("gemini-3-flash");
        assert_eq!(s.name, "8on22-bw");
    }

    #[test]
    fn resolve_shape_unknown_defaults_to_anthropic() {
        let s = resolve_shape("unknown-provider-model-xyz");
        assert_eq!(s.name, "11on16-bw");
    }

    #[test]
    fn chars_per_frame_scales_with_frame_size_and_cell() {
        // 11on16-bw: 1568px / 11 cell_width.
        let s = lookup_owned("11on16-bw").unwrap();
        assert_eq!(s.chars_per_frame(), 1568 / 11);
    }

    #[test]
    fn normalize_strips_ansi() {
        let input = "\u{1b}[31mhello\u{1b}[0m world";
        let out = normalize(input);
        assert!(!out.contains('\u{1b}'));
        assert!(out.contains("hello"));
        assert!(out.contains("world"));
    }

    #[test]
    fn normalize_folds_newlines_to_full_block() {
        let input = "line1\n\n\nline2";
        let out = normalize(input);
        assert!(out.contains('\u{2588}'));
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn normalize_replaces_box_drawing_with_ascii() {
        let input = "┌──┐\n│hi│\n└──┘";
        let out = normalize(input);
        assert!(!out.contains('┌'));
        assert!(out.contains('-'));
    }

    #[test]
    fn normalize_collapses_whitespace_runs() {
        let input = "a    b\t\tc";
        let out = normalize(input);
        assert!(!out.contains("    "));
        assert!(!out.contains('\t'));
        assert_eq!(out, "a b c");
    }

    #[test]
    fn prepare_respects_text_limit() {
        let long = "x".repeat(10_000);
        let prep = prepare(&long, 2000, 200);
        assert!(prep.bounded_text.chars().count() <= 200);
        assert!(prep.remaining_text.chars().count() >= 10_000 - 200);
    }

    #[test]
    fn prepare_short_input_has_no_remaining() {
        let prep = prepare("hello world", 2000, 200);
        assert_eq!(prep.bounded_text, "hello world\n");
        assert!(prep.remaining_text.is_empty());
    }

    #[test]
    fn serialize_conversation_truncates_tool_results() {
        let input = "Tool: a very long output that goes on and on and on";
        let text = serialize_conversation(input, 20);
        assert!(text.contains("[truncated]"));
        assert!(text.chars().count() < 60);
    }

    #[test]
    fn compact_emits_one_frame_per_chunk_with_png_bytes() {
        // Pre-port this asserted `f.bytes.is_empty()` (the old
        // NoopRenderer path). compact() now renders real PNGs, so
        // each frame should carry a non-empty payload with the
        // PNG magic header. The `frames.len() <= max_frames`
        // invariant is preserved.
        let long: String = "a".repeat(800);
        let prep = prepare(&long, 2000, 800);
        let opts = CompactOptions {
            model_id: "claude-3-5-sonnet".into(),
            ..Default::default()
        };
        let result = compact(&prep, &opts);
        assert!(!result.frames.is_empty());
        assert!(result.frames.len() <= opts.max_frames as usize);
        for f in &result.frames {
            assert!(
                !f.bytes.is_empty(),
                "frame {} should have real PNG bytes",
                f.index
            );
            assert_eq!(
                &f.bytes[..8],
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
                "frame {} missing PNG magic",
                f.index
            );
        }
    }

    #[test]
    fn compact_summary_includes_lead_in_and_head() {
        let text = "User: hello\nAssistant: hi";
        let prep = prepare(text, 2000, 2000);
        let opts = CompactOptions {
            model_id: "claude".into(),
            ..Default::default()
        };
        let result = compact(&prep, &opts);
        assert!(result.summary.starts_with("Resume prior conversation."));
    }
    #[test]
    fn compact_frame_ranges_are_disjoint_and_cover_source() {
        // Each frame holds `chars_per_frame` characters; with max_frames=5
        // and an unbounded text the frames cover exactly the first
        // `max_frames * chars_per_frame` characters.
        let long: String = (0..200).map(|i| format!("x{i}\n")).collect();
        let prep = prepare(&long, 5000, 5000);
        let opts = CompactOptions {
            model_id: "claude".into(),
            max_frames: 5,
            ..Default::default()
        };
        let result = compact(&prep, &opts);
        assert!(!result.frames.is_empty());
        // Frame 0 starts at 0; frames are contiguous and disjoint.
        let mut prev_end = 0;
        for (i, f) in result.frames.iter().enumerate() {
            assert_eq!(f.index, i as u32);
            assert_eq!(f.source_start, prev_end);
            assert!(f.source_end > f.source_start);
            prev_end = f.source_end;
        }
        // The frames cover the first `max_frames * chars_per_frame`
        // characters of the bounded text — less than the full length
        // when `max_frames * chars_per_frame` is small.
        let chars_per_frame = result.shape.chars_per_frame() as usize;
        assert_eq!(prev_end, (opts.max_frames as usize) * chars_per_frame);
    }

    #[test]
    fn bounded_source_drop_is_lossless() {
        let text = "x".repeat(2000);
        let prep = prepare(&text, 5000, 2000);
        let opts = CompactOptions {
            model_id: "claude".into(),
            ..Default::default()
        };
        let result = compact(&prep, &opts);
        let bounded = result.into_bounded_source();
        assert_eq!(bounded.text, prep.bounded_text);
    }

    #[test]
    fn compact_renders_real_png_bytes_per_frame() {
        // The compact() path used to go through a NoopRenderer that
        // emitted empty bytes; this guards against regressions to
        // that fake-success path by asserting each frame carries a
        // non-empty PNG payload.
        let long: String = "a".repeat(500);
        let prep = prepare(&long, 2000, 500);
        let opts = CompactOptions {
            model_id: "claude".into(),
            max_frames: 3,
            ..Default::default()
        };
        let result = compact(&prep, &opts);
        assert!(!result.frames.is_empty(), "compact must produce ≥1 frame");
        for f in &result.frames {
            assert!(!f.bytes.is_empty(), "frame {} has empty bytes", f.index);
            // PNG magic header.
            assert_eq!(
                &f.bytes[..8],
                &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
                "frame {} is not a PNG",
                f.index
            );
        }
    }
}
