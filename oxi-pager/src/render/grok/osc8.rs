//! Link detection for the scrollback render pass.
//!
//! Provides [`LinkOverlay`] / [`OverlayLink`] (link positions collected during
//! rendering) and [`scan_lines_for_url_overlays`] for detecting plain-text URLs
//! and absolute file paths across all block types. The collected links are
//! handed to the terminal as `LinkSpan`s and emitted as OSC 8 hyperlinks by the
//! frame diff (see `xai_ratatui_inline::Terminal::flush_with_links`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use linkify::{LinkFinder, LinkKind};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

/// Semantic destination of a pager link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Url(Arc<str>),
    File(Arc<Path>),
}

/// Whether the painted text can independently identify its semantic target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinkPresentation {
    #[default]
    Opaque,
    SelfResolvingPath,
}

/// Output and activation policy for a semantic link target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLinkTarget {
    /// Terminal-owned OSC 8 destination, or `None` when plain text owns discovery.
    pub osc8_url: Option<Arc<str>>,
    /// App-owned activation target, or `None` when activation is delegated.
    pub open_target: Option<LinkTarget>,
}

/// Resolve one semantic target using the current terminal context.
pub fn resolve_link_target(target: &LinkTarget) -> Option<ResolvedLinkTarget> {
    resolve_link_target_with_presentation(target, LinkPresentation::Opaque)
}

pub fn resolve_link_target_with_presentation(
    target: &LinkTarget,
    presentation: LinkPresentation,
) -> Option<ResolvedLinkTarget> {
    resolve_link_target_for_context(
        target,
        presentation,
        crate::render::grok::terminal::terminal_context(),
    )
}

/// Resolve one semantic target for both OSC 8 output and app-owned activation.
pub fn resolve_link_target_for_context(
    target: &LinkTarget,
    presentation: LinkPresentation,
    terminal: &crate::render::grok::terminal::TerminalContext,
) -> Option<ResolvedLinkTarget> {
    match target {
        LinkTarget::Url(url) => {
            let filter = crate::render::grok::terminal::hyperlinks::SchemeFilter::Standard;
            crate::render::grok::link_opener::is_safe_to_open(url, filter).then(|| {
                ResolvedLinkTarget {
                    osc8_url: Some(Arc::clone(url)),
                    open_target: Some(LinkTarget::Url(Arc::clone(url))),
                }
            })
        }
        LinkTarget::File(_)
            if terminal.brand == crate::render::grok::terminal::TerminalName::VsCode
                && terminal.is_official_vscode_remote
                && presentation == LinkPresentation::SelfResolvingPath =>
        {
            Some(ResolvedLinkTarget {
                osc8_url: None,
                open_target: None,
            })
        }
        LinkTarget::File(path) => Some(ResolvedLinkTarget {
            osc8_url: file_path_to_url(path),
            open_target: Some(LinkTarget::File(Arc::clone(path))),
        }),
    }
}

/// Resolve the target for app-owned activation.
pub fn resolve_link_open_target(target: &LinkTarget) -> Option<LinkTarget> {
    resolve_link_target(target).and_then(|resolved| resolved.open_target)
}

/// A single link region on screen.
#[derive(Debug, Clone)]
pub struct OverlayLink {
    pub screen_row: u16,
    pub col_start: u16,
    pub col_end: u16,
    pub target: LinkTarget,
    pub presentation: LinkPresentation,
    pub id: Option<u32>,
}

/// Accumulates link positions for post-flush OSC 8 emission.
#[derive(Debug, Clone)]
pub struct LinkOverlay {
    links: Vec<OverlayLink>,
}

impl Default for LinkOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkOverlay {
    pub fn new() -> Self {
        Self { links: Vec::new() }
    }

    pub fn push(&mut self, link: OverlayLink) {
        debug_assert!(
            link.col_start <= link.col_end,
            "OverlayLink col_start ({}) > col_end ({})",
            link.col_start,
            link.col_end
        );
        if link.col_start > link.col_end {
            return; // Silently skip inverted ranges in release mode.
        }
        self.links.push(link);
    }

    /// Append all links from `other` (clones each `OverlayLink`).
    ///
    /// Each link is routed through [`Self::push`] so the
    /// `col_start <= col_end` invariant is enforced (inverted ranges are
    /// silently dropped in release builds, just like the single-link path).
    pub fn extend_from(&mut self, other: &LinkOverlay) {
        self.links.reserve(other.links.len());
        for link in &other.links {
            self.push(link.clone());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn links(&self) -> &[OverlayLink] {
        &self.links
    }

    /// Returns `true` if an existing link overlaps the given screen region.
    pub fn overlaps(&self, screen_row: u16, col_start: u16, col_end: u16) -> bool {
        self.links
            .iter()
            .any(|l| l.screen_row == screen_row && l.col_start < col_end && col_start < l.col_end)
    }
}

fn link_finder() -> &'static LinkFinder {
    static FINDER: OnceLock<LinkFinder> = OnceLock::new();
    FINDER.get_or_init(|| {
        let mut f = LinkFinder::new();
        f.kinds(&[LinkKind::Url]);
        f
    })
}

/// One path segment without spaces (`main.rs`, `.grok`, `@scope`). Leading `.`
/// matches dot-directories and `%` matches percent-encoded segments — grok
/// session media lives under `~/.grok/sessions/%2F…/images/1.jpg`.
const PATH_SEGMENT: &str = r"[a-zA-Z0-9_@.%][a-zA-Z0-9._+@%\-]*";

/// Final path segment may contain *internal* spaces for macOS app bundles and
/// similarly named files (`Demo App.app`). Requires a `.ext` suffix
/// after the last space so trailing prose (`…/bar here.`) is not consumed.
const PATH_SEGMENT_SPACED: &str =
    r"[a-zA-Z0-9_@.%][a-zA-Z0-9._+@%\-]*(?: [a-zA-Z0-9._+@%\-]+)+\.[a-zA-Z0-9][a-zA-Z0-9._+@%\-]*";

/// Relative file path (`images/1.png`, `.grok/x.txt`) — one or more `/`-joined
/// directory segments plus a filename that has an extension. No leading `/`
/// or `~` (those are the absolute forms). The required extension keeps
/// slashed prose ("and/or", "TCP/IP") out; the caller still gates on the file
/// existing under `cwd`.
fn relative_file_path_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pat = format!(
            r"(?:{seg}/)+[a-zA-Z0-9_@%+\-]+(?:\.[a-zA-Z0-9_@%+\-]+)+",
            seg = PATH_SEGMENT,
        );
        regex::Regex::new(&pat).expect("relative file path regex")
    })
}

fn file_path_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Absolute (`/Users/me/x.md`) or home-relative (`~/Desktop/x.md`) paths.
        // Leading `~` is expanded to $HOME when building the `file://` URL.
        //
        // The *final* segment may include internal spaces when it looks like a
        // filename with an extension (tutor report: `…/Demo App.app`
        // only linkified up to the space). Intermediate segments stay
        // space-free so `…/bar here.` does not eat the word `here`.
        // Alternation prefers the spaced form first so it wins over the shorter
        // no-space prefix at the same start position.
        let pat = format!(
            r"~?/(?:{seg}/)+(?:{spaced}|{seg})",
            seg = PATH_SEGMENT,
            spaced = PATH_SEGMENT_SPACED,
        );
        regex::Regex::new(&pat).expect("file path regex")
    })
}

/// Paths wrapped in single or double quotes, including spaces in any segment.
/// Group 1 is the opening quote; group 2 is the path (no surrounding quotes).
/// Caller must verify the character immediately after the path is the same quote.
fn quoted_file_path_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Opening quote + path; closing quote checked in code (regex crate has
        // no backreferences). Path allows spaces in segments; at least two
        // `/`-separated components required.
        // `"/Users/me/My Dir/file.app"` or `'~/Desktop/My Notes/todo.md'`
        let seg = r#"[^/"']+"#;
        let pat = format!(r#"(["'])(~?/(?:{seg}/)+{seg})"#);
        regex::Regex::new(&pat).expect("quoted file path regex")
    })
}

/// Turn a display path (`/abs/…` or `~/…`) into a semantic filesystem target.
/// Relative paths fail — use [`tool_path_file_target`] to join cwd first.
pub fn path_to_file_target(path: &str) -> Option<LinkTarget> {
    tool_path_file_target(path, None)
}

fn file_path_to_url(path: &Path) -> Option<Arc<str>> {
    url::Url::from_file_path(path)
        .ok()
        .map(|u| Arc::from(u.as_str()))
}

#[cfg(test)]
fn tool_path_file_target_with_home(
    path: &str,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Option<LinkTarget> {
    let target = crate::render::grok::tool_paths::resolve_tool_path_target_with_home(
        Path::new(path),
        cwd,
        home,
    )?;
    Some(LinkTarget::File(Arc::from(target)))
}

/// Semantic target for a Read/Edit path, joining ordinary relative paths to `cwd`.
pub fn tool_path_file_target(path: &str, cwd: Option<&Path>) -> Option<LinkTarget> {
    crate::render::grok::tool_paths::resolve_tool_path_target(path, cwd)
        .map(|path| LinkTarget::File(Arc::from(path)))
}

fn file_link_presentation_for_resolved(
    painted: &str,
    target: &LinkTarget,
    cwd: Option<&Path>,
    resolved: Option<&Path>,
) -> LinkPresentation {
    let LinkTarget::File(target_path) = target else {
        return LinkPresentation::Opaque;
    };
    let painted_path = Path::new(painted);
    let is_absolute = painted_path.is_absolute()
        || matches!(
            painted_path.components().next(),
            Some(std::path::Component::Prefix(_))
        );
    let is_home_relative =
        painted == "~" || painted.starts_with("~/") || painted.starts_with(r"~\");
    if !is_absolute && !is_home_relative && (!painted.contains(['/', '\\']) || cwd.is_none()) {
        return LinkPresentation::Opaque;
    }
    resolved
        .filter(|resolved| *resolved == target_path.as_ref())
        .map_or(LinkPresentation::Opaque, |_| {
            LinkPresentation::SelfResolvingPath
        })
}

/// Classify painted file text only when it independently resolves to `target`.
pub fn file_link_presentation(
    painted: &str,
    target: &LinkTarget,
    cwd: Option<&Path>,
) -> LinkPresentation {
    let resolved = crate::render::grok::tool_paths::resolve_tool_path_target(painted, cwd);
    file_link_presentation_for_resolved(painted, target, cwd, resolved.as_deref())
}

#[cfg(test)]
fn file_link_presentation_with_home(
    painted: &str,
    target: &LinkTarget,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> LinkPresentation {
    let resolved = crate::render::grok::tool_paths::resolve_tool_path_target_with_home(
        Path::new(painted),
        cwd,
        home,
    );
    file_link_presentation_for_resolved(painted, target, cwd, resolved.as_deref())
}

/// Resolve a markdown link destination that names a local file into a semantic
/// filesystem target, so model paths (`[videos/1.mp4](videos/1.mp4)`) open on click.
///
/// Web/scheme URLs, `mailto:`/`tel:`, and anchors return `None`.
///
/// - **Absolute / `~`** paths resolve directly (must be an existing file).
/// - **Relative** paths (`images/1.jpg`) resolve against `media_paths` — the
///   absolute paths of media actually generated in this transcript — by
///   matching a unique entry whose path ends with those components. This ties
///   each short path to the exact file its message produced (correct across
///   forks/resumes) and never opens an arbitrary or out-of-session file; an
///   ambiguous or absent match is left unlinked.
pub fn local_link_to_file_target(dest: &str, media_paths: &[PathBuf]) -> Option<LinkTarget> {
    let dest = dest.trim();
    if dest.is_empty() || dest.starts_with('#') || dest.contains("://") {
        return None;
    }
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("mailto:") || lower.starts_with("tel:") {
        return None;
    }
    let path = Path::new(dest);
    let target = crate::render::grok::tool_paths::resolve_tool_path_target(dest, None)?;
    let resolved: PathBuf = if target.is_absolute() {
        target
    } else {
        // Relative: match a single generated-media file ending with these
        // components. Unique match only, so a forked transcript with a duplicate
        // name resolves to neither rather than the wrong one.
        let mut hits = media_paths.iter().filter(|p| p.ends_with(path));
        let first = hits.next()?.clone();
        if hits.next().is_some() {
            return None;
        }
        first
    };
    if !resolved.is_file() {
        return None;
    }
    Some(LinkTarget::File(Arc::from(resolved)))
}

/// Convert a display-cell column to a `u16` suitable for overlay coordinates.
///
/// Returns `None` when the column (plus content offset) would overflow
/// `u16`, in which case the caller should skip the link.
fn to_overlay_col(content_x: u16, col: usize) -> Option<u16> {
    let col16 = u16::try_from(col).ok()?;
    content_x.checked_add(col16)
}

/// One visual row of a logical (pre-wrap) line: its screen row plus the byte
/// range its text occupies within the joined logical string.
struct RowSegment {
    screen_row: u16,
    start: usize,
    end: usize,
}

/// Scan ratatui [`Line`]s for plain-text URLs and file paths, appending
/// corresponding [`OverlayLink`] entries to the overlay.
///
/// Runs on all blocks. For markdown blocks, existing hyperlinks are
/// already in the overlay; detected links that overlap are skipped.
///
/// Each item is `(screen_row, line, joiner)` where `joiner` is the soft-wrap
/// joiner to the *previous* row (see `BlockLine::joiner`): `None` = hard
/// break, `Some("")` = mid-word wrap, `Some(" ")` = word wrap. Consecutive
/// rows connected by `Some(..)` joiners are re-joined into one logical line
/// before matching, so a long path or URL soft-wrapped across rows (imagine
/// media lives at `~/.grok/sessions/%2F…/images/1.jpg`, which wraps in
/// narrow panes) is detected whole and each row's fragment gets its own
/// clickable overlay region. Spans within a row are likewise concatenated so
/// styling boundaries never truncate a match.
///
/// Detects three kinds of links:
/// 1. **URLs** via the `linkify` crate (http, https, mailto).
/// 2. **Absolute and `~`-relative file paths** via regex, emitted as
///    `file://` URLs (a leading `~/` is expanded to the home directory).
/// 3. **Relative file paths** (`images/1.png`) that uniquely match a generated
///    media file in `media_paths` — so prose like "and/or" is never linkified.
pub fn scan_lines_for_url_overlays<'a>(
    lines: impl Iterator<Item = (u16, &'a Line<'static>, Option<&'a str>)>,
    content_x: u16,
    media_paths: &[PathBuf],
    overlay: &mut LinkOverlay,
) {
    // Joined text + row segments for the logical line currently being
    // accumulated. Buffers are reused across groups to avoid per-row
    // allocation on every render frame.
    let mut group_text = String::new();
    let mut group_rows: Vec<RowSegment> = Vec::new();

    for (screen_row, line, joiner) in lines {
        // A `None` joiner is a hard break: flush the current group and start
        // a new logical line. (A `Some` joiner with no accumulated rows —
        // e.g. a wrap continuation scrolled in at the top of the viewport —
        // also starts a new group; its fragment is scanned standalone.)
        match joiner {
            Some(j) if !group_rows.is_empty() => group_text.push_str(j),
            _ => {
                scan_logical_line(&group_text, &group_rows, content_x, media_paths, overlay);
                group_text.clear();
                group_rows.clear();
            }
        }
        let start = group_text.len();
        for span in &line.spans {
            group_text.push_str(span.content.as_ref());
        }
        group_rows.push(RowSegment {
            screen_row,
            start,
            end: group_text.len(),
        });
    }
    scan_logical_line(&group_text, &group_rows, content_x, media_paths, overlay);
}

/// Push one [`OverlayLink`] per visual row that `match_range` (a byte range in
/// the joined logical `text`) overlaps.
///
/// Returns `true` if at least one overlay region was pushed. Returns `false`
/// without pushing anything when any row segment would overlap an existing
/// overlay link (e.g. a markdown hyperlink already mapped for that region) or
/// when a column exceeds `u16`.
fn push_link_segments(
    text: &str,
    rows: &[RowSegment],
    content_x: u16,
    match_range: std::ops::Range<usize>,
    target: &LinkTarget,
    presentation: LinkPresentation,
    overlay: &mut LinkOverlay,
) -> bool {
    let mut segments: Vec<(u16, u16, u16)> = Vec::new();
    for row in rows {
        // Intersect the match with this row's byte range; joiner bytes
        // between rows belong to no row and are clamped away.
        let start = match_range.start.max(row.start);
        let end = match_range.end.min(row.end);
        if start >= end {
            continue;
        }
        let col_start = UnicodeWidthStr::width(&text[row.start..start]);
        let col_end = col_start + UnicodeWidthStr::width(&text[start..end]);
        let (Some(cs), Some(ce)) = (
            to_overlay_col(content_x, col_start),
            to_overlay_col(content_x, col_end),
        ) else {
            return false;
        };
        if overlay.overlaps(row.screen_row, cs, ce) {
            return false;
        }
        segments.push((row.screen_row, cs, ce));
    }
    if segments.is_empty() {
        return false;
    }
    for (screen_row, col_start, col_end) in segments {
        overlay.push(OverlayLink {
            screen_row,
            col_start,
            col_end,
            target: target.clone(),
            presentation,
            id: None,
        });
    }
    true
}

/// Run URL / file-path detection over one joined logical line and emit
/// per-row overlay regions for every match (see [`push_link_segments`]).
fn scan_logical_line(
    text: &str,
    rows: &[RowSegment],
    content_x: u16,
    media_paths: &[PathBuf],
    overlay: &mut LinkOverlay,
) {
    if text.is_empty() || rows.is_empty() {
        return;
    }
    let scheme_filter = crate::render::grok::terminal::hyperlinks::SchemeFilter::Standard;
    let finder = link_finder();
    let path_re = file_path_regex();
    let quoted_path_re = quoted_file_path_regex();
    let rel_path_re = relative_file_path_regex();

    // Byte ranges consumed by URL links, populated lazily on first safe URL
    // hit to avoid allocation in the common case of lines with no URLs.
    let mut url_byte_ranges: Option<Vec<std::ops::Range<usize>>> = None;
    // Byte ranges already turned into file-path overlays (quoted first, then
    // plain) so later passes do not double-link.
    let mut path_byte_ranges: Vec<std::ops::Range<usize>> = Vec::new();

    for link in finder.links(text) {
        let url = link.as_str();
        if !crate::render::grok::link_opener::is_safe_to_open(url, scheme_filter) {
            continue;
        }
        url_byte_ranges
            .get_or_insert_with(Vec::new)
            .push(link.start()..link.end());

        let target = LinkTarget::Url(Arc::from(url));
        push_link_segments(
            text,
            rows,
            content_x,
            link.start()..link.end(),
            &target,
            LinkPresentation::Opaque,
            overlay,
        );
    }

    let range_overlaps_urls = |start: usize, end: usize| -> bool {
        url_byte_ranges
            .as_ref()
            .is_some_and(|ranges| ranges.iter().any(|r| start < r.end && r.start < end))
    };

    // Pass 1: quoted paths — spaces allowed in every segment.
    for caps in quoted_path_re.captures_iter(text) {
        let open_q = caps.get(1).expect("open quote");
        let path_m = caps.get(2).expect("path group");
        // Require a matching closing quote immediately after the path.
        let close_idx = path_m.end();
        if text.as_bytes().get(close_idx) != Some(&open_q.as_str().as_bytes()[0]) {
            continue;
        }
        if range_overlaps_urls(path_m.start(), path_m.end()) {
            continue;
        }
        let Some(file_target) = path_to_file_target(path_m.as_str()) else {
            continue;
        };

        // Clickable region is the path text only (not the quotes).
        if push_link_segments(
            text,
            rows,
            content_x,
            path_m.start()..path_m.end(),
            &file_target,
            file_link_presentation(path_m.as_str(), &file_target, None),
            overlay,
        ) {
            path_byte_ranges.push(path_m.start()..path_m.end());
        }
    }

    // Pass 2: unquoted paths (final segment may include spaces + ext).
    for m in path_re.find_iter(text) {
        if range_overlaps_urls(m.start(), m.end())
            || path_byte_ranges
                .iter()
                .any(|r| m.start() < r.end && r.start < m.end())
        {
            continue;
        }
        if m.start() > 0 {
            let prev = text.as_bytes()[m.start() - 1];
            if prev.is_ascii_alphanumeric()
                || matches!(prev, b'_' | b'.' | b'+' | b'@' | b'-' | b':' | b'/' | b'~')
            {
                continue;
            }
        }
        // Drop trailing sentence punctuation so a path ending a sentence
        // (`…/images/1.jpg.`) links to the file, not `file.jpg.`.
        let path = m
            .as_str()
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
        if path.is_empty() {
            continue;
        }
        let path_end = m.start() + path.len();
        let Some(file_target) = path_to_file_target(path) else {
            continue;
        };

        if push_link_segments(
            text,
            rows,
            content_x,
            m.start()..path_end,
            &file_target,
            file_link_presentation(path, &file_target, None),
            overlay,
        ) {
            path_byte_ranges.push(m.start()..path_end);
        }
    }

    // Pass 3: relative paths that uniquely match a generated media file
    // (so bare `word/word.ext` prose is not over-linkified).
    if !media_paths.is_empty() {
        for m in rel_path_re.find_iter(text) {
            if range_overlaps_urls(m.start(), m.end())
                || path_byte_ranges
                    .iter()
                    .any(|r| m.start() < r.end && r.start < m.end())
            {
                continue;
            }
            if m.start() > 0 {
                let prev = text.as_bytes()[m.start() - 1];
                if prev.is_ascii_alphanumeric()
                    || matches!(
                        prev,
                        b'_' | b'.' | b'+' | b'@' | b'-' | b':' | b'/' | b'~' | b'%'
                    )
                {
                    continue;
                }
            }
            let path = m
                .as_str()
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            let Some(file_target) = local_link_to_file_target(path, media_paths) else {
                continue;
            };
            let path_end = m.start() + path.len();

            if push_link_segments(
                text,
                rows,
                content_x,
                m.start()..path_end,
                &file_target,
                LinkPresentation::Opaque,
                overlay,
            ) {
                path_byte_ranges.push(m.start()..path_end);
            }
        }
    }
}

// OXI-CHANGE: upstream `mod tests` stripped — see NOTICE-vendored.md.
