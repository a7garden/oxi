//! Curated list of popular MCP servers that can be added with one keystroke.
//!
//! Inspired by opencode's `/mcp` preset gallery
//! ([opencode.ai/docs/mcp-servers/](https://opencode.ai/docs/mcp-servers/))
//! and the broader MCP ecosystem. Each preset is a static
//! [`McpPreset`](self::McpPreset) that pre-fills an [`EditableServer`] when
//! the user picks it from the Quick Add panel.
//!
//! # When to update
//!
//! Presets are hand-curated. When a new "obvious" MCP server becomes
//! widely-adopted (modelcontextprotocol/servers, third-party hosted
//! services, etc.), add it here. The set is intentionally small — these
//! are one-tap installation, not a directory.
//!
//! # Format reminder
//!
//! See `oxi-agent/src/mcp/types.rs` for the underlying
//! [`ServerEntry`](oxi_agent::mcp::ServerEntry) schema. Common fields:
//!
//! - `command` + `args` for stdio transport
//! - `url` + `headers` for HTTP transport
//! - `lifecycle`: `lazy` (default), `eager`, or `keep-alive`
//! - `direct_tools`: register tools as first-class agent tools (Phase 3)
//! - `env`: env vars for stdio (rendered as a placeholder in the form)
//! - `headers`: HTTP headers (rendered as a placeholder in the form)

use oxi_agent::mcp::ServerEntry;

/// A curated MCP server template shown in the Quick Add gallery.
///
/// The user picks a preset, the form opens with these fields pre-filled,
/// and they hit `Enter` on `[Save & Apply]` to add the server.
#[derive(Debug, Clone)]
pub struct McpPreset {
    /// Display name (also used as the default `ServerEntry` key).
    pub name: &'static str,
    /// Short, one-line description shown in the gallery list.
    pub summary: &'static str,
    /// Tag like "stdio", "http", "oauth" — used for filtering / grouping.
    pub tag: &'static str,
    /// The pre-filled server entry. May be stdio (command + args) or
    /// HTTP (url). Fields that need user input (env values, API keys)
    /// are intentionally left as `None` and surfaced via
    /// [`env_placeholder`](Self::env_placeholder).
    pub entry: ServerEntry,
    /// Placeholder text shown in the `Env` / `Headers` field of the edit
    /// form so the user knows which keys they need to provide. `None`
    /// when no env / headers are required.
    pub env_placeholder: Option<&'static str>,
    /// Whether the preset requires OAuth (e.g. Sentry). When `true`, the
    /// Quick Add panel shows an "OAuth" badge so the user knows to
    /// authenticate after saving.
    pub requires_oauth: bool,
}

/// The curated preset list. Stable order: most popular first.
///
/// Backed by a lazy static so the underlying `ServerEntry` values can
/// be built at runtime (their destructors aren't `const`-safe, so a
/// plain `const` array doesn't work). Use [`presets`] directly — it
/// resolves to a `&'static [McpPreset]` on first access.
pub fn presets() -> &'static [McpPreset] {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Vec<McpPreset>> = OnceLock::new();
    CACHE.get_or_init(|| {
        vec![
            // ── Local (stdio) — npm-installed reference servers ─────
            McpPreset {
                name: "filesystem",
                summary: "Read / write / search files in a directory (official MCP server).",
                tag: "stdio",
                entry: ServerEntry {
                    command: Some("npx".to_string()),
                    args: Some(vec![
                        "-y".to_string(),
                        "@modelcontextprotocol/server-filesystem".to_string(),
                        ".".to_string(),
                    ]),
                    ..ServerEntry::default()
                },
                env_placeholder: None,
                requires_oauth: false,
            },
            McpPreset {
                name: "everything",
                summary: "Reference server: prompts, resources, tools — good for testing.",
                tag: "stdio",
                entry: ServerEntry {
                    command: Some("npx".to_string()),
                    args: Some(vec![
                        "-y".to_string(),
                        "@modelcontextprotocol/server-everything".to_string(),
                    ]),
                    ..ServerEntry::default()
                },
                env_placeholder: None,
                requires_oauth: false,
            },
            McpPreset {
                name: "git",
                summary: "Read / search / manipulate local Git repositories.",
                tag: "stdio",
                entry: ServerEntry {
                    command: Some("uvx".to_string()),
                    args: Some(vec!["mcp-server-git".to_string()]),
                    ..ServerEntry::default()
                },
                env_placeholder: None,
                requires_oauth: false,
            },
            // ── Hosted (HTTP) — zero-install, browser-auth ───────────
            McpPreset {
                name: "context7",
                summary: "Resolve library IDs and pull up-to-date docs into context.",
                tag: "http",
                entry: ServerEntry {
                    url: Some("https://mcp.context7.com/mcp".to_string()),
                    ..ServerEntry::default()
                },
                env_placeholder: Some("CONTEXT7_API_KEY=…"),
                requires_oauth: false,
            },
            McpPreset {
                name: "gh_grep",
                summary: "Grep by Vercel — search public code snippets on GitHub.",
                tag: "http",
                entry: ServerEntry {
                    url: Some("https://mcp.grep.app".to_string()),
                    ..ServerEntry::default()
                },
                env_placeholder: None,
                requires_oauth: false,
            },
            McpPreset {
                name: "sentry",
                summary: "Query Sentry issues, projects, and error events.",
                tag: "http+oauth",
                entry: ServerEntry {
                    url: Some("https://mcp.sentry.dev/mcp".to_string()),
                    ..ServerEntry::default()
                },
                env_placeholder: None,
                requires_oauth: true,
            },
        ]
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

impl McpPreset {
    /// Stable, sorted-by-name list — useful for tests.
    #[cfg(test)]
    pub fn sorted() -> Vec<&'static McpPreset> {
        let mut v: Vec<&'static McpPreset> = presets().iter().collect();
        v.sort_by(|a, b| a.name.cmp(b.name));
        v
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_unique_names() {
        let mut seen = std::collections::HashSet::new();
        for p in presets() {
            assert!(seen.insert(p.name), "duplicate preset name: {}", p.name);
        }
    }

    #[test]
    fn presets_have_transport_field() {
        for p in presets() {
            let has_stdio = p.entry.command.is_some();
            let has_http = p.entry.url.is_some();
            assert!(
                has_stdio ^ has_http,
                "preset {} must set exactly one of command / url",
                p.name
            );
        }
    }

    #[test]
    fn presets_have_short_name() {
        for p in presets() {
            assert!(p.name.len() <= 24, "preset name too long: {}", p.name);
            assert!(!p.name.is_empty(), "preset name empty");
        }
    }

    #[test]
    fn preset_tags_are_recognized() {
        for p in presets() {
            assert!(
                matches!(p.tag, "stdio" | "http" | "http+oauth" | "oauth"),
                "preset {} has unknown tag: {}",
                p.name,
                p.tag
            );
        }
    }

    #[test]
    fn oauth_presets_have_url() {
        for p in presets() {
            if p.requires_oauth {
                assert!(
                    p.entry.url.is_some(),
                    "oauth preset {} must use http transport",
                    p.name
                );
            }
        }
    }

    #[test]
    fn sorted_iter_is_ordered() {
        let sorted = McpPreset::sorted();
        for w in sorted.windows(2) {
            assert!(w[0].name <= w[1].name);
        }
    }
}
