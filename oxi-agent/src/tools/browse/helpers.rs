//! Shared helpers for browser tools.
//!
//! Centralizes JS snippet generation and result parsing so that
//! `BrowseTool`, `BrowseExtractTool`, and `BrowseScriptTool` share
//! a single source of truth for DOM interaction logic.

use crate::tools::ToolError;
use crate::tools::browse::engine::BrowserTab;
use serde_json::Value;

// ── Link extraction ───────────────────────────────────────────────

/// JS that returns `[{ text, href }, …]` for every `<a href>` on the page.
pub const JS_ALL_LINKS: &str = r#"(function() {
    var links = document.querySelectorAll('a[href]');
    return Array.from(links).map(function(a) {
        return { text: a.textContent.trim(), href: a.href };
    });
})()"#;

/// JS that returns links inside the element matching `selector`.
pub fn js_links_within(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var root = document.querySelector({sel});
            if (!root) return [];
            var links = root.querySelectorAll('a[href]');
            return Array.from(links).map(function(a) {{
                return {{ text: a.textContent.trim(), href: a.href }};
            }});
        }})()"#
    )
}

/// Parse the JSON array returned by the link-extraction snippets.
pub fn parse_link_values(value: Value) -> Vec<(String, String)> {
    let Value::Array(arr) = value else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let href = item.get("href")?.as_str()?.to_string();
            if href.is_empty() {
                return None;
            }
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some((text, href))
        })
        .collect()
}

/// Extract all links from the already-loaded page (no navigation).
pub async fn extract_links(tab: &dyn BrowserTab) -> Result<Vec<(String, String)>, ToolError> {
    let value = tab
        .evaluate(JS_ALL_LINKS)
        .await
        .map_err(|e| e.to_string())?;
    Ok(parse_link_values(value))
}

/// Format link pairs as a numbered markdown list.
pub fn format_links(links: &[(String, String)]) -> String {
    links
        .iter()
        .enumerate()
        .map(|(i, (text, href))| {
            if text.is_empty() {
                format!("{}. {}", i + 1, href)
            } else {
                format!("{}. [{}]({})", i + 1, text, href)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Element extraction ────────────────────────────────────────────

/// JS that returns `[{ tag, text, attributes }]` for every element
/// matching `selector`.
pub fn js_query_elements(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var els = document.querySelectorAll({sel});
            return Array.from(els).map(function(el) {{
                var attrs = {{}};
                for (var i = 0; i < el.attributes.length; i++) {{
                    attrs[el.attributes[i].name] = el.attributes[i].value;
                }}
                return {{ tag: el.tagName, text: el.textContent.trim(), attributes: attrs }};
            }});
        }})()"#
    )
}

/// Parse the JSON array returned by `js_query_elements`.
pub fn parse_element_values(
    value: Value,
) -> Vec<(String, String, std::collections::HashMap<String, String>)> {
    let Value::Array(arr) = value else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let tag = item.get("tag")?.as_str()?.to_string();
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let attributes = item
                .get("attributes")
                .and_then(|v| v.as_object())
                .map(|map| {
                    map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            Some((tag, text, attributes))
        })
        .collect()
}

// ── DOM interaction JS builders ───────────────────────────────────

/// JS to set a `<select>` value and fire the `change` event.
pub fn js_set_select_value(selector: &str, value: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    let val = serde_json::to_string(value).unwrap_or_default();
    format!(
        r#"(function() {{
            var sel = document.querySelector({sel});
            if (!sel) throw new Error('Element not found: ' + {sel});
            sel.value = {val};
            sel.dispatchEvent(new Event('change', {{ bubbles: true }}));
        }})()"#
    )
}

/// JS to check a checkbox (only if not already checked).
pub fn js_check(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) throw new Error('Element not found: ' + {sel});
            if (!el.checked) el.click();
        }})()"#
    )
}

/// JS to uncheck a checkbox (only if currently checked).
pub fn js_uncheck(selector: &str) -> String {
    let sel = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) throw new Error('Element not found: ' + {sel});
            if (el.checked) el.click();
        }})()"#
    )
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_link_values_empty() {
        assert!(parse_link_values(Value::Null).is_empty());
        assert!(parse_link_values(Value::Array(vec![])).is_empty());
    }

    #[test]
    fn test_parse_link_values_filters_empty_href() {
        let input = serde_json::json!([
            { "text": "ok", "href": "https://x.com" },
            { "text": "bad", "href": "" },
            { "text": "also ok", "href": "https://y.com" }
        ]);
        let links = parse_link_values(input);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].1, "https://x.com");
        assert_eq!(links[1].1, "https://y.com");
    }

    #[test]
    fn test_format_links() {
        let links = vec![
            ("Hello".to_string(), "https://a.com".to_string()),
            ("".to_string(), "https://b.com".to_string()),
        ];
        let out = format_links(&links);
        assert!(out.contains("[Hello](https://a.com)"));
        assert!(out.contains("2. https://b.com"));
    }

    #[test]
    fn test_js_query_elements_contains_selector() {
        let js = js_query_elements(".item");
        assert!(js.contains(".item"));
        assert!(js.contains("querySelectorAll"));
        assert!(js.contains("textContent"));
    }

    #[test]
    fn test_js_links_within_scopes_to_selector() {
        let js = js_links_within("#nav");
        assert!(js.contains("#nav"));
        assert!(js.contains("querySelector"));
        assert!(js.contains("querySelectorAll('a[href]')"));
    }

    #[test]
    fn test_parse_element_values() {
        let input = serde_json::json!([
            { "tag": "DIV", "text": "hello", "attributes": { "class": "item" } }
        ]);
        let elems = parse_element_values(input);
        assert_eq!(elems.len(), 1);
        assert_eq!(elems[0].0, "DIV");
        assert_eq!(elems[0].1, "hello");
        assert_eq!(elems[0].2.get("class").unwrap(), "item");
    }
}
