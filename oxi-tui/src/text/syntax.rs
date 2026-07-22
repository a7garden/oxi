//! Optional syntect-based code highlighting. Enable with `feature = "syntax"`.
//!
//! Without the feature, [`SyntaxHighlighter`] is a zero-cost stub that returns
//! each code line as a plain monospace [`Line`]. `StreamingMarkdown` falls back
//! to this stub when the binary is built without syntax support, keeping the
//! default `oxi-tui` binary small.
//!
//! With the feature enabled, [`SyntaxHighlighter::new`] loads the bundled
//! syntax and theme packs on first use, and [`SyntaxHighlighter::highlight`]
//! renders `code` tagged with `lang` into a vector of styled [`Line`]s.

use ratatui::text::Line;
#[cfg(feature = "syntax")]
use ratatui::{
    style::{Color, Style},
    text::Span,
};

/// Syntax highlighter for fenced code blocks.
///
/// The same type is exported under both feature flags. When `syntax` is
/// enabled, instances cache a [`SyntaxSet`] and [`ThemeSet`] so repeated
/// highlight calls are cheap. When the feature is off, the struct is a
/// zero-sized stub and `highlight` returns plain lines.
#[cfg(feature = "syntax")]
pub struct SyntaxHighlighter {
    syntax_set: syntect::parsing::SyntaxSet,
    theme_set: syntect::highlighting::ThemeSet,
    /// Name of the active theme. Defaults to `"base16-ocean.dark"` when the
    /// bundled theme pack is present.
    theme_name: String,
}

/// Plain monospace fallback used when the `syntax` feature is disabled.
#[cfg(not(feature = "syntax"))]
pub struct SyntaxHighlighter;

#[cfg(feature = "syntax")]
impl SyntaxHighlighter {
    /// Load the bundled syntax and theme packs. This is moderately expensive
    /// (a few MB of static data is parsed into lookup tables), so callers
    /// should share a single instance across the renderer.
    #[must_use]
    pub fn new() -> Self {
        let syntax_set = syntect::parsing::SyntaxSet::load_defaults_newlines();
        let theme_set = syntect::highlighting::ThemeSet::load_defaults();
        // `base16-ocean.dark` ships in the default theme pack; fall back to
        // any bundled theme if a future upstream change removes it.
        let theme_name = if theme_set.themes.contains_key("base16-ocean.dark") {
            "base16-ocean.dark".to_owned()
        } else {
            theme_set.themes.keys().next().cloned().unwrap_or_default()
        };
        Self {
            syntax_set,
            theme_set,
            theme_name,
        }
    }

    /// Highlight a code block, returning one [`Line`] per source line.
    ///
    /// `lang` is a [`pulldown-cmark`]-style language tag (e.g. `"rust"`,
    /// `"python"`). Unknown or empty tags fall through to plain text. Lines
    /// are emitted with foreground colors sourced from the active theme;
    /// background and UI chrome stay under the host [`crate::theme::Theme`].
    #[must_use]
    pub fn highlight(&self, code: &str, lang: &str) -> Vec<Line<'static>> {
        let Some(theme) = self.theme_set.themes.get(&self.theme_name) else {
            return plain_lines(code);
        };

        let syntax = if lang.is_empty() {
            self.syntax_set.find_syntax_by_extension("txt")
        } else {
            self.syntax_set
                .find_syntax_by_token(lang)
                .or_else(|| self.syntax_set.find_syntax_by_extension(lang))
                .or_else(|| self.syntax_set.find_syntax_by_name(lang))
        }
        .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(code.lines().count());
        for line in syntect::util::LinesWithEndings::from(code) {
            let ranges = highlighter.highlight_line(line, &self.syntax_set);
            // A failed highlight still emits the source text unstyled rather
            // than dropping the line — the user keeps their code.
            // Each `LinesWithEndings` slice ends with `\n`, and the highlighter
            // puts that newline on the last range. Strip it so each ratatui
            // `Line` (one terminal row) does not embed a literal newline in
            // its span content.
            let spans = match ranges {
                Ok(ranges) => {
                    let len = ranges.len();
                    ranges
                        .into_iter()
                        .enumerate()
                        .map(|(idx, (style, text))| {
                            let cleaned = if idx + 1 == len {
                                text.trim_end_matches(['\n', '\r']).to_owned()
                            } else {
                                text.to_owned()
                            };
                            Span::styled(cleaned, convert_style(style))
                        })
                        .collect()
                }
                Err(_) => vec![Span::raw(line.trim_end_matches(['\n', '\r']).to_owned())],
            };
            lines.push(Line::from(spans));
        }
        lines
    }
}

#[cfg(not(feature = "syntax"))]
impl SyntaxHighlighter {
    /// Construct the plain fallback highlighter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Return one [`Line`] per source line, each styled as plain text.
    #[must_use]
    pub fn highlight(&self, code: &str, _lang: &str) -> Vec<Line<'static>> {
        plain_lines(code)
    }
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Build plain monospace lines from raw source. Newlines are stripped so
/// each ratatui `Line` (one terminal row) does not embed a literal newline
/// in its span content — matches the syntax path's behavior.
fn plain_lines(code: &str) -> Vec<Line<'static>> {
    code.lines().map(|s| Line::from(s.to_owned())).collect()
}

/// Convert a syntect [`syntect::highlighting::Style`] into a ratatui
/// [`Style`]. Only foreground color is forwarded — backgrounds and font
/// styles are intentionally left out so the surrounding chrome (theme
/// panels, code backgrounds) keeps full control.
#[cfg(feature = "syntax")]
fn convert_style(syn_style: syntect::highlighting::Style) -> Style {
    let fg = Color::Rgb(
        syn_style.foreground.r,
        syn_style.foreground.g,
        syn_style.foreground.b,
    );
    let mut style = Style::default().fg(fg);
    let fs = syn_style.font_style;
    if fs.contains(syntect::highlighting::FontStyle::BOLD) {
        style = style.add_modifier(ratatui::style::Modifier::BOLD);
    }
    if fs.contains(syntect::highlighting::FontStyle::ITALIC) {
        style = style.add_modifier(ratatui::style::Modifier::ITALIC);
    }
    if fs.contains(syntect::highlighting::FontStyle::UNDERLINE) {
        style = style.add_modifier(ratatui::style::Modifier::UNDERLINED);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "syntax")]
    #[test]
    fn highlight_rust_code() {
        let hl = SyntaxHighlighter::new();
        let code = "fn main() {\n    println!(\"hi\");\n}\n";
        let lines = hl.highlight(code, "rust");
        assert_eq!(lines.len(), 3, "should produce one line per source line");

        // Concatenated span content matches the source with newlines stripped.
        let rendered: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(rendered, "fn main() {    println!(\"hi\");}");

        // At least one span on the function definition should carry a
        // non-default foreground color (the `fn` keyword).
        let fn_line = &lines[0];
        assert!(
            fn_line.spans.iter().any(|span| span.style.fg.is_some()),
            "expected some styled fg on the first line"
        );
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn highlight_unknown_lang_falls_back_to_plain_text() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("hello\nworld\n", "definitely-not-a-language");
        assert_eq!(lines.len(), 2);
        let rendered: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(rendered, "helloworld");
    }

    #[cfg(not(feature = "syntax"))]
    #[test]
    fn fallback_returns_plain_lines() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("fn main() {\n    println!(\"hi\");\n}\n", "rust");
        assert_eq!(lines.len(), 3);
        let rendered: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(rendered, "fn main() {    println!(\"hi\");}");
        for line in &lines {
            for span in &line.spans {
                assert!(
                    span.style.fg.is_none(),
                    "fallback must not emit any foreground color"
                );
            }
        }
    }

    #[cfg(not(feature = "syntax"))]
    #[test]
    fn fallback_handles_empty_input() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("", "rust");
        assert!(lines.is_empty());
    }
}
