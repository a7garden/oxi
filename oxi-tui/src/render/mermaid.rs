//! Mermaid diagram rendering to terminal ASCII art.
//!
//! Detects the [`mmdc`](https://github.com/mermaid-js/mermaid-cli) (mermaid-cli)
//! binary on `$PATH` and uses it to convert Mermaid source into ASCII diagrams
//! for terminal display. Results are cached per source+options so repeated
//! renders of the same diagram are instant.
//!
//! When `mmdc` is unavailable, rendering returns `None` and callers fall back
//! to displaying the raw Mermaid source as a fenced code block.

use std::collections::HashMap;
use std::process::Command;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
use which::which;

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Color mode for rendered Mermaid diagrams.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
pub enum MermaidColorMode {
    /// No color — plain ASCII output. This is the default.
    #[default]
    None,
    /// Theme-aware colored output.
    Themed,
}

/// Options that control how Mermaid diagrams are rendered.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MermaidRenderOptions {
    /// Color mode for the rendered diagram.
    pub color_mode: MermaidColorMode,
    /// Whether to request ASCII-art output (as opposed to SVG/PNG).
    pub use_ascii: bool,
    /// Maximum terminal width in columns for viewport adaptation.
    ///
    /// When `None`, no width constraint is applied.
    pub max_width: Option<u16>,
}

impl Default for MermaidRenderOptions {
    /// Defaults: no color, ASCII output, no width limit.
    fn default() -> Self {
        Self {
            color_mode: MermaidColorMode::None,
            use_ascii: true,
            max_width: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Process-level cache for Mermaid renders.
///
/// Keyed by source text + options; stores `None` for failed renders so they
/// are not retried on every frame redraw.
static MERMAID_CACHE: LazyLock<RwLock<HashMap<String, Option<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Monotonic counter for generating unique temp-file names.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render Mermaid `source` to ASCII art using the `mmdc` CLI.
///
/// Checks `$PATH` for the `mmdc` (mermaid-cli) binary. If found, writes the
/// source to a temp file, runs `mmdc --outputFormat ascii`, and reads back the
/// result. Results are cached so repeated calls with identical source and
/// options return instantly.
///
/// Returns `None` when:
///
/// - `source` is empty or whitespace-only,
/// - `mmdc` is not installed,
/// - the external process fails.
///
/// Callers should fall back to displaying the raw code block on `None`.
pub fn render_mermaid_ascii(source: &str, options: &MermaidRenderOptions) -> Option<String> {
    let normalized = source.replace("\r\n", "\n");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }

    let key = cache_key(normalized, options);

    // Cache hit — return immediately (including cached failures).
    if let Some(cached) = MERMAID_CACHE.read().get(&key) {
        return cached.clone();
    }

    let rendered = render_with_mmdc(normalized);
    MERMAID_CACHE.write().insert(key, rendered.clone());
    rendered
}

/// Clear all cached Mermaid renders.
///
/// After calling this, every diagram will be re-rendered from scratch on the
/// next call to [`render_mermaid_ascii`].
pub fn clear_mermaid_cache() {
    MERMAID_CACHE.write().clear();
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build a cache key from source text and options.
fn cache_key(source: &str, options: &MermaidRenderOptions) -> String {
    format!(
        "{}\x00{:?}\x00{}",
        source, options.color_mode, options.use_ascii
    )
}

/// Invoke the external `mmdc` CLI to render `source` to ASCII.
///
/// Returns `None` if `mmdc` is absent or the process fails. Temp files are
/// cleaned up regardless of outcome.
fn render_with_mmdc(source: &str) -> Option<String> {
    let mmdc = which("mmdc").ok()?;

    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("oxi-mermaid-{id}"));
    let input_path = base.with_extension("mmd");
    let output_path = base.with_extension("txt");

    std::fs::write(&input_path, source).ok()?;

    let result = (|| {
        let output = Command::new(&mmdc)
            .arg("-i")
            .arg(&input_path)
            .arg("-o")
            .arg(&output_path)
            .arg("-t")
            .arg("default")
            .arg("--outputFormat")
            .arg("ascii")
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        std::fs::read_to_string(&output_path).ok()
    })();

    // Clean up temp files.
    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&output_path);

    result
}

/// Measure the display width of the widest line in an ASCII string.
fn ascii_display_width(ascii: &str) -> usize {
    ascii.lines().map(UnicodeWidthStr::width).max().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Box rendering
// ---------------------------------------------------------------------------

/// Render an ASCII diagram string inside a titled box border.
///
/// Produces [`Line`]s with box-drawing characters wrapping the ASCII content,
/// suitable for use in a `Paragraph`. The border is dimmed so the diagram
/// content stands out.
///
/// # Example output
///
/// ```text
/// ┌─ mermaid ─────┐
/// │  A──►B        │
/// │  B──►C        │
/// └───────────────┘
/// ```
pub fn render_ascii_diagram(ascii: &str) -> Vec<Line<'static>> {
    let border = Style::new().add_modifier(Modifier::DIM);
    // 9 = display width of " mermaid " (the title text).
    let w = ascii_display_width(ascii).max(9);

    let mut lines = Vec::new();

    // Top border: ┌─ mermaid ─...─┐
    let title_fill = w.saturating_sub(9);
    lines.push(Line::from(vec![
        Span::styled("┌─", border),
        Span::styled(" mermaid ", border),
        Span::styled("─".repeat(title_fill), border),
        Span::styled("┐", border),
    ]));

    // Content rows: │ content padded │
    for line in ascii.lines() {
        let line_w = UnicodeWidthStr::width(line);
        let pad = w.saturating_sub(line_w);
        lines.push(Line::from(vec![
            Span::styled("│ ", border),
            Span::raw(line.to_string()),
            Span::raw(" ".repeat(pad)),
            Span::styled("│", border),
        ]));
    }

    // Bottom border: └─...─┘
    lines.push(Line::from(vec![
        Span::styled("└", border),
        Span::styled("─".repeat(w + 1), border),
        Span::styled("┘", border),
    ]));

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_returns_none() {
        let opts = MermaidRenderOptions::default();
        assert_eq!(render_mermaid_ascii("", &opts), None);
        assert_eq!(render_mermaid_ascii("   \n  ", &opts), None);
    }

    #[test]
    fn cache_returns_same_result() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let source = "sequenceDiagram\n  A->>B: hello";
        // mmdc likely absent in test env → both None (cached).
        let result1 = render_mermaid_ascii(source, &opts);
        let result2 = render_mermaid_ascii(source, &opts);
        assert_eq!(result1, result2);
    }

    #[test]
    fn clear_cache_empties_cache() {
        clear_mermaid_cache();
        let opts = MermaidRenderOptions::default();
        let _ = render_mermaid_ascii("graph TD\n  A-->B", &opts);
        assert!(!MERMAID_CACHE.read().is_empty());
        clear_mermaid_cache();
        assert!(MERMAID_CACHE.read().is_empty());
    }

    #[test]
    fn ascii_display_width_ascii() {
        assert_eq!(ascii_display_width("hello"), 5);
        assert_eq!(ascii_display_width("ab\ncd"), 2);
        assert_eq!(ascii_display_width(""), 0);
    }

    #[test]
    fn ascii_display_width_cjk() {
        // CJK characters are double-width.
        assert_eq!(ascii_display_width("한글"), 4);
    }

    #[test]
    fn render_ascii_diagram_has_borders() {
        let lines = render_ascii_diagram("hello");
        // Top border + one content line + bottom border.
        assert_eq!(lines.len(), 3);
        // Top border starts with ┌.
        assert!(lines[0].spans.iter().any(|s| s.content.starts_with("┌")));
        // Bottom border ends with ┘.
        assert!(lines[2].spans.iter().any(|s| s.content.ends_with('┘')));
    }

    #[test]
    fn render_ascii_diagram_empty_input() {
        let lines = render_ascii_diagram("");
        // No content lines — just top + bottom.
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn render_ascii_diagram_multiline() {
        let lines = render_ascii_diagram("A\nBB\nCCC");
        // 3 content lines + 2 borders.
        assert_eq!(lines.len(), 5);
        // Inner width is max(3, 9) = 9 (minimum for " mermaid " title).
        // Bottom border: └ + (w+1) dashes + ┘ → 9+1 = 10 dashes.
        let bottom_dashes: String = lines[4]
            .spans
            .iter()
            .flat_map(|s| s.content.chars())
            .filter(|&c| c == '─')
            .collect();
        assert_eq!(bottom_dashes.chars().count(), 10);
    }
}
