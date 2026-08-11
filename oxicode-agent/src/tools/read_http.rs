//! Lightweight HTTP reader-mode fetch for the `read` tool.
//!
//! When `read` is given an `http://`/`https://` path, this module fetches the
//! body with `reqwest` and converts HTML into reader-mode markdown. There is
//! **no JavaScript rendering and no browser engine** — it works in every
//! build (default features). For dynamic / JS-rendered pages, screenshots, or
//! interaction, the agent should use `browse` (the `native-browser` feature).
//!
//! Role split: `read <url>` = fast static body; `browse <url>` = full browser.

use std::time::Duration;

use super::ToolError;

/// Maximum response body size we will read/convert (2 MiB).
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// HTTP fetch + redirect/timeout budget (seconds).
const FETCH_TIMEOUT_SECS: u64 = 15;

/// A fetched page, ready to hand back to the agent as tool output.
pub(crate) struct PageFetch {
    /// Final URL after redirects.
    pub url: String,
    /// Page `<title>` (empty if none / non-HTML).
    pub title: String,
    /// Reader-mode markdown (HTML), pretty JSON (JSON), or raw text.
    pub content: String,
    /// The `Content-Type` header value (lowercased), for metadata.
    pub content_type: String,
}

/// Fetch a URL and return reader-mode content.
///
/// Errors on non-2xx status, transport failure, or timeout. Bodies larger than
/// [`MAX_BYTES`] are truncated.
pub(crate) async fn fetch_url(url: &str) -> Result<PageFetch, ToolError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("oxicode/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("http client build failed: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("fetch failed for {url}: {e}"))?;

    let status = resp.status();
    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    if !status.is_success() {
        return Err(format!("HTTP {} {}", status.as_u16(), url));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed reading body for {url}: {e}"))?;
    let cap = bytes.len().min(MAX_BYTES);
    // from_utf8_lossy tolerates a truncated multibyte tail.
    let raw = String::from_utf8_lossy(&bytes[..cap]).into_owned();

    let (content, title) = if content_type.contains("html") {
        let (md, title) = html_to_markdown(&raw);
        (md, title)
    } else if content_type.contains("json") {
        (pretty_json(&raw), String::new())
    } else {
        (raw, String::new())
    };

    Ok(PageFetch {
        url: final_url,
        title,
        content,
        content_type,
    })
}

/// Best-effort HTML → reader-mode markdown conversion.
///
/// Strips non-content blocks (script/style/head/nav/footer/aside/form/svg/
/// template/noscript), converts common block/inline elements to markdown, then
/// decodes HTML entities and collapses excess whitespace. No DOM — a small
/// sequential tag scanner. "Good enough" for static docs/blogs/articles; not
/// expected to handle arbitrary JS-rendered SPAs.
fn html_to_markdown(html: &str) -> (String, String) {
    let title = extract_title(html);
    let stripped = strip_noncontent_blocks(html);
    let converted = convert_tags(&stripped);
    let decoded = decode_entities(&converted);
    (collapse_whitespace(&decoded), title)
}

/// Extract the first `<title>…</title>` text (case-insensitive, trimmed).
fn extract_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let start = match lower.find("<title") {
        Some(s) => s,
        None => return String::new(),
    };
    let after_tag = match lower[start..].find('>') {
        Some(r) => start + r + 1,
        None => return String::new(),
    };
    let end = match lower[after_tag..].find("</title>") {
        Some(r) => after_tag + r,
        None => html.len(),
    };
    decode_entities(html[after_tag..end].trim())
}

/// Remove entire `<tag …> … </tag>` blocks for non-content elements.
/// Operates case-insensitively on the tag name; matches the *next* close tag
/// of the same name (no nesting handling — adequate for well-formed pages).
fn strip_noncontent_blocks(html: &str) -> String {
    const BLOCKS: &[&str] = &[
        "script", "style", "noscript", "head", "nav", "footer", "aside", "form", "svg", "template",
        "iframe",
    ];
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < html.len() {
        if bytes[i] == b'<' {
            // Is this an opening tag of a blocked element?
            if let Some(tag_end) = lower[i..].find('>') {
                let tag_inner = &lower[i + 1..i + tag_end];
                let name = tag_inner
                    .split(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
                    .next()
                    .unwrap_or("");
                if BLOCKS.contains(&name) {
                    let close = format!("</{name}");
                    if let Some(rel) = lower[i..].find(&close) {
                        let after = lower[i..][rel..].find('>').map(|r| i + rel + r + 1);
                        i = after.unwrap_or(html.len());
                        continue;
                    } else {
                        // Unclosed block — drop to end.
                        break;
                    }
                }
            }
        }
        // Not a blocked open tag — copy this char.
        // Advance by one UTF-8 char to stay on boundaries.
        let ch_end = next_char_boundary(html, i);
        out.push_str(&html[i..ch_end]);
        i = ch_end;
    }
    out
}

/// Next UTF-8 char boundary at or after byte index `i`.
fn next_char_boundary(s: &str, i: usize) -> usize {
    let mut j = i + 1;
    while j < s.len() && !s.is_char_boundary(j) {
        j += 1;
    }
    j.min(s.len())
}

/// Sequential tag scanner: convert known HTML tags to markdown, drop the rest.
fn convert_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut pending_href: Option<String> = None;
    let mut i = 0;
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            // Comment?
            if lower[i..].starts_with("<!--") {
                if let Some(rel) = lower[i..].find("-->") {
                    i += rel + 3;
                } else {
                    i = html.len();
                }
                continue;
            }
            let tag_end = match lower[i..].find('>') {
                Some(r) => i + r,
                None => {
                    out.push('&');
                    i += 1;
                    continue;
                }
            };
            let inner = &html[i + 1..tag_end];
            let lname = &lower[i + 1..tag_end];
            let is_close = lname.starts_with('/');
            let name_raw = if is_close { &lname[1..] } else { lname };
            let name = name_raw
                .split(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
                .next()
                .unwrap_or("");
            let self_closing = lname.ends_with('/');

            emit_tag(
                name,
                is_close,
                self_closing,
                inner,
                &mut out,
                &mut pending_href,
            );
            i = tag_end + 1;
        } else {
            let ch_end = next_char_boundary(html, i);
            out.push_str(&html[i..ch_end]);
            i = ch_end;
        }
    }
    out
}

/// Emit the markdown equivalent for a single tag (or nothing for unknown).
fn emit_tag(
    name: &str,
    is_close: bool,
    _self_closing: bool,
    inner: &str,
    out: &mut String,
    pending_href: &mut Option<String>,
) {
    match name {
        "br" => out.push('\n'),
        "hr" => out.push_str("\n\n---\n\n"),
        "h1" if !is_close => out.push_str("\n\n# "),
        "h2" if !is_close => out.push_str("\n\n## "),
        "h3" if !is_close => out.push_str("\n\n### "),
        "h4" if !is_close => out.push_str("\n\n#### "),
        "h5" if !is_close => out.push_str("\n\n##### "),
        "h6" if !is_close => out.push_str("\n\n###### "),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => out.push('\n'),
        "p" | "div" | "section" | "article" | "header" | "main" | "tr" if !is_close => {
            out.push_str("\n\n")
        }
        "li" if !is_close => out.push_str("\n- "),
        "pre" if !is_close => out.push_str("\n```\n"),
        "pre" => out.push_str("\n```\n"),
        "blockquote" if !is_close => out.push_str("\n> "),
        "strong" | "b" => out.push_str("**"),
        "em" | "i" => out.push('*'),
        "code" => out.push('`'),
        "a" if !is_close => {
            pending_href.take(); // reset
            if let Some(href) = extract_attr(inner, "href")
                && !href.trim().is_empty()
            {
                *pending_href = Some(href);
            }
            out.push('[');
        }
        "a" => {
            // closing </a>
            if let Some(href) = pending_href.take() {
                out.push_str("](");
                out.push_str(&href);
                out.push(')');
            } else {
                out.push(']');
            }
        }
        "img" => {
            if let Some(src) = extract_attr(inner, "src") {
                let alt = extract_attr(inner, "alt").unwrap_or_default();
                out.push_str("\n\n![");
                out.push_str(&alt);
                out.push_str("](");
                out.push_str(&src);
                out.push_str(")\n\n");
            }
        }
        _ => {} // unknown/inline tags: drop the tag, keep inner text
    }
}

/// Extract an attribute value (double- or single-quoted, or bare) from a tag's
/// inner text. Returns the decoded value if present.
fn extract_attr(tag_inner: &str, attr: &str) -> Option<String> {
    let lower = tag_inner.to_ascii_lowercase();
    let pat = format!("{attr}=");
    let idx = lower.find(&pat)?;
    let after = idx + pat.len();
    let rest = &tag_inner[after..];
    let val = if let Some(stripped) = rest.strip_prefix('"') {
        stripped.split('"').next().unwrap_or("")
    } else if let Some(stripped) = rest.strip_prefix('\'') {
        stripped.split('\'').next().unwrap_or("")
    } else {
        rest.split(|c: char| c.is_ascii_whitespace())
            .next()
            .unwrap_or("")
    };
    let decoded = decode_entities(val);
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

/// Decode common HTML entities (named + numeric decimal/hex).
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if bytes[i] == b'&'
            && let Some(semi) = s[i..].find(';')
        {
            let ent = &s[i + 1..i + semi];
            if let Some(c) = decode_one(ent) {
                out.push(c);
                i += semi + 1;
                continue;
            }
        }
        let ch_end = next_char_boundary(s, i);
        out.push_str(&s[i..ch_end]);
        i = ch_end;
    }
    out
}

/// Decode a single entity body (without the surrounding `&` `;`).
fn decode_one(ent: &str) -> Option<char> {
    let c = match ent {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00A0}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "eacute" => 'é',
        "egrave" => 'è',
        "agrave" => 'à',
        "ccedil" => 'ç',
        "uuml" => 'ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "szlig" => 'ß',
        "laquo" => '«',
        "raquo" => '»',
        "rsquo" => '’',
        "lsquo" => '‘',
        "ldquo" => '“',
        "rdquo" => '”',
        _ => return decode_numeric(ent),
    };
    Some(c)
}

/// Decode a numeric entity body: `#NN` (decimal) or `#xHH` (hex).
fn decode_numeric(ent: &str) -> Option<char> {
    if let Some(hex) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
        u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
    } else if let Some(dec) = ent.strip_prefix('#') {
        dec.parse::<u32>().ok().and_then(char::from_u32)
    } else {
        None
    }
}

/// Collapse runs of 3+ newlines to 2, trim trailing spaces per line, trim ends.
fn collapse_whitespace(s: &str) -> String {
    // Collapse runs of 3+ newlines to 2 (paragraph spacing); preserve single
    // and double newlines (needed for <pre>/code blocks and paragraphs).
    let mut collapsed = String::with_capacity(s.len());
    let mut nl = 0;
    for c in s.chars() {
        if c == '\n' {
            nl += 1;
            if nl <= 2 {
                collapsed.push('\n');
            }
        } else {
            nl = 0;
            collapsed.push(c);
        }
    }
    // Trim trailing spaces per line, then the whole string.
    collapsed
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Pretty-print JSON if it parses; otherwise return the raw string.
fn pretty_json(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw.trim()) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()),
        Err(_) => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_basic_structure() {
        let html = "<html><head><title>Hello World</title></head>\
                    <body><nav>Home</nav>\
                    <h1>Title</h1><p>First <strong>bold</strong> para.</p>\
                    <ul><li>one</li><li>two</li></ul>\
                    <a href=\"https://e.com\">link</a></body></html>";
        let (md, title) = html_to_markdown(html);
        assert_eq!(title, "Hello World");
        assert!(md.contains("# Title"));
        assert!(md.contains("**bold**"));
        assert!(md.contains("- one"));
        assert!(md.contains("- two"));
        assert!(md.contains("[link](https://e.com)"));
        // nav stripped
        assert!(!md.contains("Home"));
    }

    #[test]
    fn strips_script_and_style() {
        let html = "<p>keep</p><script>alert('x')</script><style>.a{}</style><p>also</p>";
        let (md, _) = html_to_markdown(html);
        assert!(md.contains("keep"));
        assert!(md.contains("also"));
        assert!(!md.contains("alert"));
        assert!(!md.contains(".a{}"));
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &quot;q&quot;"),
            "a & b <c> \"q\""
        );
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        assert_eq!(decode_entities("caf&eacute;"), "café");
    }

    #[test]
    fn collapses_excess_whitespace() {
        let html = "<p>a</p>\n\n\n\n\n<p>b</p>";
        let (md, _) = html_to_markdown(html);
        assert!(!md.contains("\n\n\n"));
        assert!(md.contains("a\n\nb"));
    }

    #[test]
    fn unknown_tags_dropped_inner_kept() {
        let html = "<span>x</span><custom>y</custom>";
        let (md, _) = html_to_markdown(html);
        // Unknown inline tags are dropped, inner text kept; no block
        // separation is synthesized for inline elements.
        assert_eq!(md, "xy");
    }

    #[test]
    fn code_and_pre() {
        let html = "<pre>line1\nline2</pre><p>use <code>foo</code> here</p>";
        let (md, _) = html_to_markdown(html);
        assert!(md.contains("```\nline1\nline2\n```"));
        assert!(md.contains("`foo`"));
    }

    #[test]
    fn img_to_markdown_image() {
        let html = "<img src=\"/a.png\" alt=\"pic\">";
        let (md, _) = html_to_markdown(html);
        assert!(md.contains("![pic](/a.png)"));
    }

    #[test]
    fn title_missing_is_empty() {
        let html = "<body><p>no title</p></body>";
        let (_, title) = html_to_markdown(html);
        assert_eq!(title, "");
    }

    #[test]
    fn pretty_json_indents() {
        let out = pretty_json("{\"a\":1}");
        assert_eq!(out, "{\n  \"a\": 1\n}");
    }
}
