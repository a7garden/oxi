//! Text wrapping utilities with style preservation.
//!
//! Provides word-aware wrapping that preserves styled spans
//! and tracks soft vs hard line breaks (via joiners).

use ratatui::text::{Line, Span};
use std::borrow::Cow;
use std::ops::Range;
use textwrap::Options;
use unicode_width::UnicodeWidthChar;

use super::line_utils::{fit_line_to_width, push_owned_lines};

/// Compute byte ranges for wrapped lines without trailing whitespace.
pub(crate) fn wrap_ranges_trim<'a, O>(text: &str, width_or_options: O) -> Vec<Range<usize>>
where
    O: Into<Options<'a>>,
{
    let opts = width_or_options.into();
    let mut lines: Vec<Range<usize>> = Vec::new();
    for line in textwrap::wrap(text, opts).iter() {
        match line {
            Cow::Borrowed(slice) => {
                let start = unsafe { slice.as_ptr().offset_from(text.as_ptr()) as usize };
                let end = start + slice.len();
                lines.push(start..end);
            }
            Cow::Owned(_) => panic!("wrap_ranges_trim: unexpected owned string"),
        }
    }
    lines
}

/// A segment of a match highlight on a single wrapped row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSegment {
    /// Wrapped row index (0-based).
    pub row: usize,
    /// Start display column within that row (0-based, unicode-width aware).
    pub col_start: usize,
    /// End display column (exclusive).
    pub col_end: usize,
}

/// Map a byte range in `text` to one or more `(row, col_start, col_end)`
/// segments, given the byte ranges per wrapped row.
///
/// `text`        — the original flat text (needed for display-width conversion).
/// `wrap_ranges` — byte ranges per wrapped row (from `wrap_ranges_trim`).
/// `match_range` — the byte range of the match in the flat text.
///
/// `col_start` and `col_end` in the returned segments are **display columns**,
/// not byte offsets.  This correctly handles multi-byte characters (e.g. em dash
/// `—` is 3 bytes but 1 display column).
///
/// Returns segments for each wrapped row that the match overlaps.
/// Empty if the match doesn't overlap any row.
pub fn byte_range_to_row_cols(
    text: &str,
    wrap_ranges: &[Range<usize>],
    match_range: Range<usize>,
) -> Vec<HighlightSegment> {
    let mut segments = Vec::new();
    for (row, wr) in wrap_ranges.iter().enumerate() {
        // Intersect match_range with this row's byte range.
        let start = match_range.start.max(wr.start);
        let end = match_range.end.min(wr.end);
        if start < end {
            // Convert byte offsets within this row to display columns.
            let row_text = &text[wr.start..wr.end];
            let col_start = byte_offset_to_display_col(row_text, start - wr.start);
            let col_end = byte_offset_to_display_col(row_text, end - wr.start);
            segments.push(HighlightSegment {
                row,
                col_start,
                col_end,
            });
        }
    }
    segments
}

/// Convert a byte offset within `text` to the corresponding display column.
///
/// Iterates characters from the start, summing their `UnicodeWidthChar` display
/// widths until the cumulative byte count reaches `byte_offset`.
pub(crate) fn byte_offset_to_display_col(text: &str, byte_offset: usize) -> usize {
    let mut col = 0usize;
    let mut bytes = 0usize;
    for ch in text.chars() {
        if bytes >= byte_offset {
            break;
        }
        bytes += ch.len_utf8();
        col += ch.width().unwrap_or(0);
    }
    col
}

/// Compute byte ranges per wrapped row using the same wrapping options
/// as [`word_wrap_line_with_joiners`].
///
/// Uses `FirstFit` algorithm and `break_words(true)` to match
/// [`RtOptions`] defaults — the critical invariant is that the wrap
/// breakpoints here match the visual rendering exactly.
///
/// `text` is the flattened plain text (must match `search_text()` for
/// correct highlight mapping).
///
/// NOTE: This uses a single-pass `textwrap::wrap` rather than mirroring
/// the two-stage (first-line + remainder) logic of `word_wrap_line_with_joiners`.
/// With `FirstFit` (greedy), single-pass produces identical breakpoints
/// because each line's break depends only on text from the current position
/// forward. If a future change to the wrapping pipeline breaks this
/// invariant, consider switching to a two-stage approach that mirrors
/// `word_wrap_line_with_joiners` exactly.
#[allow(clippy::single_range_in_vec_init)] // intentional: single range = full text, no wrapping
pub fn wrap_byte_ranges_matching(text: &str, width: usize) -> Vec<Range<usize>> {
    if width == 0 || text.is_empty() {
        return vec![0..text.len()];
    }

    // Must match the Options used by word_wrap_line_with_joiners (via RtOptions).
    // Blockquote lines get a subsequent_indent equal to the prefix, which
    // reduces the effective width for continuation lines. Mirror that here
    // so search-highlight breakpoints stay in sync with the visual rendering.
    let bq_len = blockquote_prefix_len(text);
    let subsequent_width = if bq_len > 0 && bq_len < text.len() {
        let prefix_display_width = text[..bq_len]
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .sum::<usize>();
        width.saturating_sub(prefix_display_width).max(1)
    } else {
        width
    };

    // First line wraps at full width; remainder at reduced width for blockquotes.
    let first_line_opts = textwrap::Options::new(width)
        .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit)
        .word_splitter(textwrap::WordSplitter::HyphenSplitter)
        .break_words(true);
    let first_ranges = wrap_ranges_trim(text, first_line_opts);
    let Some(first) = first_ranges.first() else {
        return vec![0..text.len()];
    };

    let mut ranges = vec![first.clone()];
    let mut base = first.end;

    // Skip whitespace at the wrap boundary (mirrors word_wrap_line_with_joiners).
    let skip = text[base..].chars().take_while(|c| *c == ' ').count();
    base = base.saturating_add(skip);

    if base < text.len() {
        let remainder = &text[base..];
        let rem_opts = textwrap::Options::new(subsequent_width)
            .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit)
            .word_splitter(textwrap::WordSplitter::HyphenSplitter)
            .break_words(true);
        for r in wrap_ranges_trim(remainder, rem_opts) {
            ranges.push((r.start + base)..(r.end + base));
        }
    }

    ranges
}

/// Wrapping options with initial/subsequent indent support.
#[derive(Debug, Clone)]
pub struct RtOptions<'a> {
    /// The width in columns at which the text will be wrapped.
    pub width: usize,
    /// Line ending used for breaking lines.
    pub line_ending: textwrap::LineEnding,
    /// Indentation used for the first line of output.
    pub initial_indent: Line<'a>,
    /// Indentation used for subsequent lines of output.
    pub subsequent_indent: Line<'a>,
    /// Allow long words to be broken if they cannot fit on a line.
    pub break_words: bool,
    /// Wrapping algorithm to use.
    pub wrap_algorithm: textwrap::WrapAlgorithm,
    /// The line breaking algorithm to use.
    pub word_separator: textwrap::WordSeparator,
    /// The method for splitting words.
    pub word_splitter: textwrap::WordSplitter,
}

impl From<usize> for RtOptions<'_> {
    fn from(width: usize) -> Self {
        RtOptions::new(width)
    }
}

#[allow(dead_code)]
impl<'a> RtOptions<'a> {
    pub fn new(width: usize) -> Self {
        RtOptions {
            width,
            line_ending: textwrap::LineEnding::LF,
            initial_indent: Line::default(),
            subsequent_indent: Line::default(),
            break_words: true,
            word_separator: textwrap::WordSeparator::new(),
            wrap_algorithm: textwrap::WrapAlgorithm::FirstFit,
            word_splitter: textwrap::WordSplitter::HyphenSplitter,
        }
    }

    pub fn line_ending(self, line_ending: textwrap::LineEnding) -> Self {
        RtOptions {
            line_ending,
            ..self
        }
    }

    pub fn width(self, width: usize) -> Self {
        RtOptions { width, ..self }
    }

    pub fn initial_indent(self, initial_indent: Line<'a>) -> Self {
        RtOptions {
            initial_indent,
            ..self
        }
    }

    pub fn subsequent_indent(self, subsequent_indent: Line<'a>) -> Self {
        RtOptions {
            subsequent_indent,
            ..self
        }
    }

    pub fn break_words(self, break_words: bool) -> Self {
        RtOptions {
            break_words,
            ..self
        }
    }

    pub fn word_separator(self, word_separator: textwrap::WordSeparator) -> RtOptions<'a> {
        RtOptions {
            word_separator,
            ..self
        }
    }

    pub fn wrap_algorithm(self, wrap_algorithm: textwrap::WrapAlgorithm) -> RtOptions<'a> {
        RtOptions {
            wrap_algorithm,
            ..self
        }
    }

    pub fn word_splitter(self, word_splitter: textwrap::WordSplitter) -> RtOptions<'a> {
        RtOptions {
            word_splitter,
            ..self
        }
    }
}

/// Wrap a single line, preserving styles.
#[must_use]
pub fn word_wrap_line<'a, O>(line: &'a Line<'a>, width_or_options: O) -> Vec<Line<'a>>
where
    O: Into<RtOptions<'a>>,
{
    let (lines, _joiners) = word_wrap_line_with_joiners(line, width_or_options);
    lines
}

fn flatten_line_and_bounds<'a>(
    line: &'a Line<'a>,
) -> (String, Vec<(Range<usize>, ratatui::style::Style)>) {
    let mut flat = String::new();
    let mut span_bounds = Vec::new();
    let mut acc = 0usize;
    for s in &line.spans {
        let text = s.content.as_ref();
        let start = acc;
        flat.push_str(text);
        acc += text.len();
        span_bounds.push((start..acc, s.style));
    }
    (flat, span_bounds)
}

fn build_wrapped_line_from_range<'a>(
    indent: Line<'a>,
    original: &'a Line<'a>,
    span_bounds: &[(Range<usize>, ratatui::style::Style)],
    range: &Range<usize>,
    cursor: &mut usize,
) -> Line<'a> {
    let mut out = indent.style(original.style);
    let sliced = slice_line_spans(original, span_bounds, range, cursor);
    let mut spans = out.spans;
    spans.append(
        &mut sliced
            .spans
            .into_iter()
            .map(|s| s.patch_style(original.style))
            .collect(),
    );
    out.spans = spans;
    out
}

/// Check if a line is a table line (box-drawing border or content row).
///
/// Table lines start with box-drawing characters and should never be word-wrapped,
/// as wrapping destroys column alignment. Instead, they are passed through as-is
/// and clipped by the terminal at the edge.
///
/// Blockquote lines also start with `│` (U+2502) but should NOT be treated as
/// table lines — they need word wrapping with the prefix repeated on continuation
/// lines.  The distinction: table content rows have interior `│` cell separators
/// (e.g. `│ cell1 │ cell2 │`), while blockquote lines have `│` only in the
/// leading prefix (e.g. `│ text` or `│ │ nested text`).
fn is_table_line(line: &Line<'_>) -> bool {
    let mut chars = line.spans.iter().flat_map(|s| s.content.chars());
    match chars.next() {
        // Box-drawing border characters (horizontal lines, corners, junctions)
        // — these are unambiguously table borders, never blockquote prefixes.
        Some(ch)
            if ('\u{2500}'..='\u{257F}').contains(&ch) && ch != '\u{2502}' && ch != '\u{2503}' =>
        {
            true
        }
        // │ (U+2502) at line start: table content row only if │ appears after
        // the leading prefix region (blockquote prefixes are only │ and spaces).
        Some('\u{2502}') => {
            let mut in_prefix = true;
            for ch in chars {
                if in_prefix && (ch == '\u{2502}' || ch == ' ') {
                    continue;
                }
                in_prefix = false;
                if ch == '\u{2502}' {
                    return true;
                }
            }
            false
        }
        // ASCII table borders (TableBorders::ASCII uses '+' and '|')
        Some('|') => true,
        _ => false,
    }
}

/// Byte length of the blockquote prefix at the start of `flat`.
///
/// A blockquote prefix is one or more `│ ` (U+2502 + space) sequences,
/// e.g. `│ ` for a single-level quote or `│ │ ` for a nested quote.
/// Returns 0 if the text does not start with a blockquote prefix.
///
/// The selection layer's stricter, style-aware twin lives in xai-grok-pager
/// scrollback/blocks/quote_bar.rs (`rendered_quote_prefix_len`); it relies on
/// this wrap layer re-injecting the prefix spans (with their styles) on
/// continuation rows, so keep the two shapes in agreement.
fn blockquote_prefix_len(flat: &str) -> usize {
    const BAR_BYTES: usize = '\u{2502}'.len_utf8(); // 3
    let mut len = 0;
    let mut chars = flat.chars();
    while let Some('\u{2502}') = chars.next() {
        if chars.next() == Some(' ') {
            len += BAR_BYTES + 1;
        } else {
            break;
        }
    }
    len
}

/// Wrap a single line and also return, for each output line, the string that should be inserted
/// when joining it to the previous output line as a *soft wrap*.
///
/// - The first output line always has `None`.
/// - Continuation lines have `Some(joiner)` where `joiner` is the exact substring (often spaces,
///   possibly empty) that was skipped at the wrap boundary.
pub fn word_wrap_line_with_joiners<'a, O>(
    line: &'a Line<'a>,
    width_or_options: O,
) -> (Vec<Line<'a>>, Vec<Option<String>>)
where
    O: Into<RtOptions<'a>>,
{
    let mut rt_opts: RtOptions<'a> = width_or_options.into();

    // Table lines must never be word-wrapped (it destroys column alignment).
    // Clip+pad each row to the content width so it owns every column; trusting
    // the terminal to clip at the edge desyncs by a column on glyphs it renders
    // wider than measured, stranding a "ghost" cell past the trailing border.
    if is_table_line(line) {
        let fitted = fit_line_to_width(line.clone(), rt_opts.width);
        return (vec![fitted], vec![None]);
    }

    let (flat, span_bounds) = flatten_line_and_bounds(line);

    // Blockquote lines start with │ (U+2502) prefix(es).  Set subsequent_indent
    // to the prefix so continuation lines repeat the quote marker.
    // Skip if the caller already set a subsequent_indent (caller intent wins).
    let bq_len = blockquote_prefix_len(&flat);
    if bq_len > 0 && bq_len < flat.len() && rt_opts.subsequent_indent.width() == 0 {
        let prefix = slice_line_spans(line, &span_bounds, &(0..bq_len), &mut 0);
        rt_opts = rt_opts.subsequent_indent(prefix);
    }

    let opts = Options::new(rt_opts.width)
        .line_ending(rt_opts.line_ending)
        .break_words(rt_opts.break_words)
        .wrap_algorithm(rt_opts.wrap_algorithm)
        .word_separator(rt_opts.word_separator)
        .word_splitter(rt_opts.word_splitter);

    let mut out: Vec<Line<'a>> = Vec::new();
    let mut joiners: Vec<Option<String>> = Vec::new();

    // The first output line uses the initial indent and a reduced available width.
    let initial_width_available = opts
        .width
        .saturating_sub(rt_opts.initial_indent.width())
        .max(1);
    let initial_wrapped = wrap_ranges_trim(&flat, opts.clone().width(initial_width_available));
    let Some(first_line_range) = initial_wrapped.first() else {
        out.push(rt_opts.initial_indent.clone());
        joiners.push(None);
        return (out, joiners);
    };

    // Shared monotonic cursor into `span_bounds`: rows are emitted in
    // increasing byte order, so each row resumes the span scan where the
    // previous one stopped instead of rescanning from 0 (see
    // `slice_line_spans`). This is what keeps wrapping one huge line linear.
    let mut span_cursor = 0usize;

    let first_line = build_wrapped_line_from_range(
        rt_opts.initial_indent.clone(),
        line,
        &span_bounds,
        first_line_range,
        &mut span_cursor,
    );
    out.push(first_line);
    joiners.push(None);

    // Wrap the remainder using subsequent indent width.
    let mut base = first_line_range.end;
    let skip_leading_spaces = flat[base..].chars().take_while(|c| *c == ' ').count();
    let joiner_first = flat[base..base.saturating_add(skip_leading_spaces)].to_string();
    base = base.saturating_add(skip_leading_spaces);

    let subsequent_width_available = opts
        .width
        .saturating_sub(rt_opts.subsequent_indent.width())
        .max(1);
    let remaining = &flat[base..];
    let remaining_wrapped = wrap_ranges_trim(remaining, opts.width(subsequent_width_available));

    let mut prev_end = 0usize;
    for (i, r) in remaining_wrapped.iter().enumerate() {
        if r.is_empty() {
            continue;
        }

        let joiner = if i == 0 {
            joiner_first.clone()
        } else {
            remaining[prev_end..r.start].to_string()
        };
        prev_end = r.end;

        let offset_range = (r.start + base)..(r.end + base);
        let subsequent_line = build_wrapped_line_from_range(
            rt_opts.subsequent_indent.clone(),
            line,
            &span_bounds,
            &offset_range,
            &mut span_cursor,
        );
        out.push(subsequent_line);
        joiners.push(Some(joiner));
    }

    (out, joiners)
}

/// Wrap a sequence of lines, applying the initial indent only to the very first
/// output line, and using the subsequent indent for all later wrapped pieces.
#[allow(private_bounds)]
pub fn word_wrap_lines<'a, I, O, L>(lines: I, width_or_options: O) -> Vec<Line<'static>>
where
    I: IntoIterator<Item = L>,
    L: IntoLineInput<'a>,
    O: Into<RtOptions<'a>>,
{
    let base_opts: RtOptions<'a> = width_or_options.into();
    let mut out: Vec<Line<'static>> = Vec::new();

    for (idx, line) in lines.into_iter().enumerate() {
        let line_input = line.into_line_input();
        let opts = if idx == 0 {
            base_opts.clone()
        } else {
            let mut o = base_opts.clone();
            let sub = o.subsequent_indent.clone();
            o = o.initial_indent(sub);
            o
        };
        let wrapped = word_wrap_line(line_input.as_ref(), opts);
        push_owned_lines(&wrapped, &mut out);
    }

    out
}

/// Like `word_wrap_lines`, but also returns a parallel vector of soft-wrap joiners.
#[allow(private_bounds)]
pub fn word_wrap_lines_with_joiners<'a, I, O, L>(
    lines: I,
    width_or_options: O,
) -> (Vec<Line<'static>>, Vec<Option<String>>)
where
    I: IntoIterator<Item = L>,
    L: IntoLineInput<'a>,
    O: Into<RtOptions<'a>>,
{
    let base_opts: RtOptions<'a> = width_or_options.into();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut joiners: Vec<Option<String>> = Vec::new();

    for (idx, line) in lines.into_iter().enumerate() {
        let line_input = line.into_line_input();
        let opts = if idx == 0 {
            base_opts.clone()
        } else {
            let mut o = base_opts.clone();
            let sub = o.subsequent_indent.clone();
            o = o.initial_indent(sub);
            o
        };

        let (wrapped, wrapped_joiners) = word_wrap_line_with_joiners(line_input.as_ref(), opts);
        for (l, j) in wrapped.into_iter().zip(wrapped_joiners) {
            out.push(super::line_utils::line_to_static(&l));
            joiners.push(j);
        }
    }

    (out, joiners)
}

/// Utilities to allow wrapping either borrowed or owned lines.
#[derive(Debug)]
enum LineInput<'a> {
    Borrowed(&'a Line<'a>),
    Owned(Line<'a>),
}

impl<'a> LineInput<'a> {
    fn as_ref(&self) -> &Line<'a> {
        match self {
            LineInput::Borrowed(line) => line,
            LineInput::Owned(line) => line,
        }
    }
}

/// This trait makes it easier to pass whatever we need into word_wrap_lines.
trait IntoLineInput<'a> {
    fn into_line_input(self) -> LineInput<'a>;
}

impl<'a> IntoLineInput<'a> for &'a Line<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Borrowed(self)
    }
}

impl<'a> IntoLineInput<'a> for &'a mut Line<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Borrowed(self)
    }
}

impl<'a> IntoLineInput<'a> for Line<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(self)
    }
}

impl<'a> IntoLineInput<'a> for String {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for &'a str {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for Cow<'a, str> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for Span<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for Vec<Span<'a>> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

/// Slice the spans of `original` down to byte `range`, using `cursor` as a
/// monotonic hint of the first span that may still be relevant.
///
/// `span_bounds` are contiguous, sorted, non-overlapping byte ranges (one per
/// span in `original`, from [`flatten_line_and_bounds`]). A line is wrapped
/// top-to-bottom, so successive `range`s have non-decreasing `start`; that lets
/// `cursor` advance permanently past spans that end before the current row
/// instead of rescanning from index 0 on every row.
///
/// Without the cursor this is O(rows × spans): each of the R wrapped rows
/// rescans all S spans — quadratic on one huge line (e.g. a long streamed
/// reasoning paragraph flattened to thousands of styled spans, the pathology
/// behind the 100%-CPU render-thread spin). With it, wrapping a line is
/// O(rows + spans).
///
/// `cursor` must start at `0` (or any index ≤ the first relevant span) and be
/// reused across the row sequence for one line. Issuing a `range` that starts
/// before a previous one is unsupported — the cursor does not rewind.
fn slice_line_spans<'a>(
    original: &'a Line<'a>,
    span_bounds: &[(Range<usize>, ratatui::style::Style)],
    range: &Range<usize>,
    cursor: &mut usize,
) -> Line<'a> {
    let start_byte = range.start;
    let end_byte = range.end;

    // Spans ending at/before this row's start are done for every later row too
    // (ranges are contiguous and queries monotonic), so advance the shared
    // cursor past them once and never revisit them.
    while span_bounds
        .get(*cursor)
        .is_some_and(|(r, _)| r.end <= start_byte)
    {
        *cursor += 1;
    }

    let mut acc: Vec<Span<'a>> = Vec::new();
    for (i, (r, style)) in span_bounds.iter().enumerate().skip(*cursor) {
        let s = r.start;
        let e = r.end;
        if s >= end_byte {
            break;
        }
        let seg_start = start_byte.max(s);
        let seg_end = end_byte.min(e);
        if seg_end > seg_start {
            let local_start = seg_start - s;
            let local_end = seg_end - s;
            let content = original.spans[i].content.as_ref();
            let slice = &content[local_start..local_end];
            acc.push(Span {
                style: *style,
                content: Cow::Borrowed(slice),
            });
        }
        if e >= end_byte {
            break;
        }
    }
    Line {
        style: original.style,
        alignment: original.alignment,
        spans: acc,
    }
}

/// Word-wrap a header line with hanging indent.
///
/// First span stays on line 1; remaining content wraps with
/// `extra_indent + prefix_width` hanging indent on continuation lines.
pub fn wrap_header_hanging(
    header: Line<'static>,
    width: usize,
    extra_indent: usize,
) -> Vec<Line<'static>> {
    if header.spans.is_empty() {
        return vec![header];
    }

    let prefix_width = unicode_width::UnicodeWidthStr::width(header.spans[0].content.as_ref());
    let total_indent = extra_indent + prefix_width;
    let wrap_width = width.saturating_sub(total_indent);

    if wrap_width == 0 || header.spans.len() < 2 {
        return word_wrap_lines(std::iter::once(header), width);
    }

    let prefix_span = header.spans[0].clone();
    let content_line = Line::from(header.spans[1..].to_vec());
    let mut wrapped = word_wrap_lines(std::iter::once(content_line), wrap_width);

    if let Some(first) = wrapped.first_mut() {
        first.spans.insert(0, prefix_span);
    }
    if total_indent > 0 {
        let indent_str: String = " ".repeat(total_indent);
        for line in wrapped.iter_mut().skip(1) {
            line.spans
                .insert(0, ratatui::text::Span::raw(indent_str.clone()));
        }
    }

    wrapped
}

/// Word-wrap a header line with continuation lines indented to `indent`.
///
/// Unlike `wrap_header_hanging` (which indents under the content after the
/// prefix), this indents continuation lines to a fixed column — typically
/// the bullet width, so wrapped text aligns with the start of the header.
pub fn wrap_header_flush(header: Line<'static>, width: usize, indent: usize) -> Vec<Line<'static>> {
    let wrap_width = width.saturating_sub(indent);
    let mut wrapped = word_wrap_lines(std::iter::once(header), wrap_width);
    if indent > 0 {
        let indent_str: String = " ".repeat(indent);
        for line in wrapped.iter_mut().skip(1) {
            line.spans
                .insert(0, ratatui::text::Span::raw(indent_str.clone()));
        }
    }
    wrapped
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
