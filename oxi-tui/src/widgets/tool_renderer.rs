//! Tool call renderer — formats tool calls and results for display.
//!
//! Provides specialized formatting for built-in tools (edit, bash, read, etc.)
//! based on the tool name, arguments JSON, and result text.
//!
//! Auto-detection: if a result looks like a unified diff, it's rendered
//! as a colored diff view even without explicit hints.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::text::truncate_to_width;
use crate::theme::ThemeStyles;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// ── Constants ────────────────────────────────────────────────────────────

/// Maximum lines to show in result preview
const RESULT_PREVIEW_LINES: usize = 5;
/// Maximum lines to show in diff view
const DIFF_PREVIEW_LINES: usize = 8;
/// Maximum lines to show for read output
const READ_PREVIEW_LINES: usize = 4;
/// Maximum content lines for generic results
const GENERIC_PREVIEW_LINES: usize = 4;

// ── Format cache ─────────────────────────────────────────────────────────

/// Soft cap on cached entries per formatter. On overflow the whole map is
/// dropped (no LRU bookkeeping) — a session rarely produces this many
/// *distinct* formatted strings, and the cost of a cold re-render is tiny.
const TOOL_FORMAT_CACHE_CAP: usize = 512;

/// Memoization cache for the two expensive tool-block formatters
/// ([`format_tool_call`] / [`format_tool_result`]).
///
/// Completed tool blocks are reformatted on every render frame even though
/// their inputs rarely change. This cache keys formatted output by a hash of
/// the formatter's inputs and returns a clone on hit, skipping the JSON parse,
/// diff auto-detection, and per-tool dispatch. omp's `CachedOutputBlock` uses
/// the same idea with a bigint hash.
///
/// The stored [`ThemeStyles`] invalidates the entire cache on a theme or
/// glyph-set change (hot-reload / `/settings`), so nothing ever renders with
/// stale colors or icons.
#[derive(Debug, Default)]
pub struct ToolFormatCache {
    /// Style bundle the cached lines were rendered with. Any change (theme
    /// hot-reload, glyph-set switch) drops every entry.
    styles: Option<ThemeStyles>,
    /// `format_tool_call` outputs, keyed by `(name, arguments, max_width)`.
    calls: HashMap<u64, Vec<Line<'static>>>,
    /// `format_tool_result` outputs, keyed by `(name, result, is_error, max_width)`.
    results: HashMap<u64, Vec<Line<'static>>>,
}

impl ToolFormatCache {
    fn key(parts: &[&str], flags: &[usize]) -> u64 {
        let mut h = DefaultHasher::new();
        for p in parts {
            p.hash(&mut h);
        }
        for f in flags {
            f.hash(&mut h);
        }
        h.finish()
    }

    fn invalidate_if_styles_changed(&mut self, styles: ThemeStyles) {
        if self.styles != Some(styles) {
            self.styles = Some(styles);
            self.calls.clear();
            self.results.clear();
        }
    }

    /// Memoized [`format_tool_call`]. Returns a clone of the cached lines when
    /// `(name, arguments, max_width)` and the active styles are unchanged.
    pub fn format_call(
        &mut self,
        styles: ThemeStyles,
        name: &str,
        arguments: &str,
        max_width: usize,
    ) -> Vec<Line<'static>> {
        self.invalidate_if_styles_changed(styles);
        let key = Self::key(&[name, arguments], &[max_width]);
        if let Some(lines) = self.calls.get(&key) {
            return lines.clone();
        }
        if self.calls.len() >= TOOL_FORMAT_CACHE_CAP {
            self.calls.clear();
        }
        let lines = format_tool_call(name, arguments, max_width, &styles);
        self.calls.insert(key, lines.clone());
        lines
    }

    /// Memoized [`format_tool_result`]. Returns a clone of the cached lines
    /// when `(name, result, is_error, arguments, max_width)` and the styles are
    /// unchanged.
    pub fn format_result(
        &mut self,
        styles: ThemeStyles,
        name: &str,
        result: &str,
        is_error: bool,
        arguments: Option<&str>,
        max_width: usize,
    ) -> Vec<Line<'static>> {
        self.invalidate_if_styles_changed(styles);
        let key = Self::key(&[name, result], &[if is_error { 1 } else { 0 }, max_width]);
        if let Some(lines) = self.results.get(&key) {
            return lines.clone();
        }
        if self.results.len() >= TOOL_FORMAT_CACHE_CAP {
            self.results.clear();
        }
        let lines = format_tool_result(name, result, is_error, arguments, max_width, &styles);
        self.results.insert(key, lines.clone());
        lines
    }
}

// ── Path utilities ───────────────────────────────────────────────────────

/// Shorten a file path by replacing home directory with ~.
pub fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str: String = home.to_string_lossy().into_owned();
        if let Some(rest) = path.strip_prefix(&*home_str) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

// ── JSON parsing helpers ──────────────────────────────────────────────────

/// Parse arguments JSON to extract common fields.
pub fn parse_tool_args(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or(Value::Null)
}

/// Extract a string field from parsed arguments.
pub fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)?.as_str()
}

/// Extract an optional path field from arguments (handles path/file_path).
pub fn get_path(args: &Value) -> Option<String> {
    args.get("path")
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())
        .map(shorten_path)
}

/// Extract an integer field from arguments.
pub fn get_int(args: &Value, key: &str) -> Option<i64> {
    args.get(key)?.as_i64()
}

// ── Auto-detection ────────────────────────────────────────────────────────

/// Check if result text looks like a unified diff.
pub fn looks_like_diff(text: &str) -> bool {
    text.lines().take(5).any(|l| {
        l.starts_with("@@ -")
            || (l.starts_with('-') && !l.starts_with("--"))
            || (l.starts_with('+') && !l.starts_with("++"))
    })
}

/// Check if result indicates a command exit status.
pub fn has_exit_status(text: &str) -> bool {
    text.contains("Command exited with code")
        || text.contains("Command timed out")
        || text.contains("Command aborted")
}

/// Count diff stats from a unified diff text.
pub fn count_diff_stats(diff: &str) -> (u32, u32) {
    let mut added = 0u32;
    let mut removed = 0u32;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("--") {
            removed += 1;
        }
    }
    (added, removed)
}

// ── Call formatters ───────────────────────────────────────────────────────

/// Resolve a tool name to its icon glyph from the active symbol set.
fn tool_icon(name: &str, sym: crate::symbols::Symbols) -> &'static str {
    match name {
        "edit" => sym.tool_edit,
        "bash" => sym.tool_bash,
        "read" => sym.tool_read,
        "write" => sym.tool_write,
        "grep" | "find" | "ls" => sym.tool_search,
        "issue" => sym.icon_search,
        "ask" => sym.tool_ask,
        "web_search" | "get_search_results" => sym.tool_web,
        "subagent" | "task" => sym.tool_task,
        "mcp" => sym.tool_mcp,
        "lsp" => sym.tool_lsp,
        "debug" => sym.tool_debug,
        "generate_image" => sym.tool_write,
        "memory_recall" | "memory_reflect" | "memory_retain" | "memory_edit" => sym.tool_task,
        "context7" => sym.icon_context,
        "github" => sym.icon_git,
        _ => sym.tool_generic,
    }
}

/// Format a tool call header line with the tool icon prepended.
fn format_call_header(name: &str, extra: &str, styles: &ThemeStyles) -> Line<'static> {
    let icon = tool_icon(name, styles.symbols);
    let name_style = styles.accent.add_modifier(Modifier::BOLD);
    let extra_style = styles.muted;
    Line::from(vec![
        Span::styled(format!("{icon} {name} "), name_style),
        Span::styled(extra.to_string(), extra_style),
    ])
}

/// Format the edit tool call.
pub fn format_edit_call(args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let path = get_path(args);
    let path_display = path.unwrap_or_else(|| "?".to_string());

    // Count edits
    let edit_count = args
        .get("edits")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or_else(|| {
            // Legacy mode: check oldText
            if args.get("oldText").or(args.get("oldText")).is_some() {
                1
            } else {
                0
            }
        });

    let extra = if edit_count > 0 {
        format!(
            "{} ({} replacement{})",
            path_display,
            edit_count,
            if edit_count == 1 { "" } else { "s" }
        )
    } else {
        path_display
    };

    vec![format_call_header("edit", &extra, styles)]
}

/// Format the bash tool call.
pub fn format_bash_call(
    args: &Value,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let command = get_str(args, "command").unwrap_or("...");
    // Account for "$ " prefix (2 chars)
    let command_display = truncate_to_width(command, max_width.saturating_sub(2).max(20));

    let mut lines = vec![Line::from(vec![
        Span::styled("$ ", styles.accent.add_modifier(Modifier::BOLD)),
        Span::styled(command_display, styles.normal),
    ])];

    // Add timeout if present
    if let Some(timeout) = get_int(args, "timeout") {
        lines.push(Line::from(Span::styled(
            format!("  (timeout {}s)", timeout),
            styles.muted,
        )));
    }

    lines
}

/// Format the read tool call.
pub fn format_read_call(args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let path = get_path(args).unwrap_or_else(|| "?".to_string());

    // Line range
    let offset = get_int(args, "offset");
    let limit = get_int(args, "limit");

    let extra = match (offset, limit) {
        (Some(o), Some(l)) => format!("{}:{}-{}", path, o, o + l - 1),
        (Some(o), None) => format!("{}:{}", path, o),
        (None, Some(l)) => format!("{}:1-{}", path, l),
        (None, None) => path,
    };

    vec![format_call_header("read", &extra, styles)]
}

/// Format the write tool call.
pub fn format_write_call(args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let path = get_path(args).unwrap_or_else(|| "?".to_string());
    let extra = format!("{} (new)", path);
    vec![format_call_header("write", &extra, styles)]
}

/// Format search tools (grep, find, ls).
pub fn format_search_call(name: &str, args: &Value, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let icon = tool_icon(name, styles.symbols);

    let path = get_path(args).unwrap_or_else(|| ".".to_string());

    // Pattern for grep
    let pattern = get_str(args, "pattern")
        .or_else(|| get_str(args, "query"))
        .map(|p| format!(" \"{}\"", truncate_to_width(p, 30)))
        .unwrap_or_default();

    let extra = format!("{}{}", pattern, path);
    vec![Line::from(vec![
        Span::styled(
            format!("{icon} {name} "),
            styles.accent.add_modifier(Modifier::BOLD),
        ),
        Span::styled(extra, styles.muted),
    ])]
}

// ── Issue tool formatter ────────────────────────────────────────────────────

/// Format the `issue` tool call. The tool has a single `action` discriminator
/// (list/read/create/update/start/release/close/link_session) and renders
/// compactly so the chat shows what's about to happen without dumping the
/// full parameter set.
pub fn format_issue_call(
    args: &Value,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("?");
    let id = args.get("id").and_then(|v| v.as_i64());

    let header = Line::from(vec![
        Span::styled(
            "issue ".to_string(),
            styles.accent.add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{action} "), styles.normal),
    ]);

    let mut lines = vec![header];
    match action {
        "create" => {
            // Title (highlighted) + priority/labels as tags on a second line.
            if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
                let title_disp = truncate_to_width(title, max_width.saturating_sub(2));
                lines.push(Line::from(vec![
                    Span::styled("  ", styles.muted),
                    Span::styled(format!("“{title_disp}”"), styles.normal),
                ]));
            }
            let mut tags: Vec<String> = Vec::new();
            if let Some(p) = args.get("priority").and_then(|v| v.as_str()) {
                tags.push(format!("[{p}]"));
            }
            if let Some(arr) = args.get("labels").and_then(|v| v.as_array()) {
                let labels: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                if !labels.is_empty() {
                    tags.push(format!("[{}]", labels.join(",")));
                }
            }
            if !tags.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", tags.join(" ")),
                    styles.muted,
                )));
            }
        }
        "list" => {
            // Show active filters as muted tag line.
            let mut filters: Vec<String> = Vec::new();
            if let Some(s) = args.get("status").and_then(|v| v.as_str()) {
                filters.push(format!("status={s}"));
            }
            if let Some(p) = args.get("priority").and_then(|v| v.as_str()) {
                filters.push(format!("priority={p}"));
            }
            if let Some(l) = args.get("label").and_then(|v| v.as_str()) {
                filters.push(format!("label={l}"));
            }
            if let Some(t) = args.get("text").and_then(|v| v.as_str()) {
                filters.push(format!("text=“{}”", truncate_to_width(t, 24)));
            }
            if filters.is_empty() {
                lines.push(Line::from(Span::styled("  (all open)", styles.muted)));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {}", filters.join("  ")),
                    styles.muted,
                )));
            }
        }
        "update" | "start" | "release" | "close" | "link_session" | "read" => {
            if let Some(i) = id {
                lines.push(Line::from(Span::styled(format!("  #{i}"), styles.muted)));
            }
            // For update, surface what fields are being changed.
            if action == "update" {
                let mut changed: Vec<&str> = Vec::new();
                if args.get("title").is_some() {
                    changed.push("title");
                }
                if args.get("body").is_some() {
                    changed.push("body");
                }
                if args.get("priority").is_some() {
                    changed.push("priority");
                }
                if args.get("status").is_some() {
                    changed.push("status");
                }
                if args.get("labels").is_some() {
                    changed.push("labels");
                }
                if !changed.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("  {} {}", styles.symbols.arrow_right, changed.join(", ")),
                        styles.muted,
                    )));
                }
            }
        }
        other => {
            lines.push(Line::from(Span::styled(
                format!("  (unknown action: {other})"),
                styles.warning,
            )));
        }
    }
    lines
}

/// Format the `issue` tool result. We pattern-match the success/error strings
/// produced by `IssueTool` (and the underlying `FileIssueStore`) so the chat
/// surfaces semantic distinctions (created vs. closed vs. conflict).
pub fn format_issue_result(
    result: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let text = result.trim();
    let first_line = text.lines().next().unwrap_or("");

    // ── Errors first — detect by leading prefix pattern ──────────────────
    if let Some(rest) = first_line.strip_prefix("issue ") {
        // Conflict / Assigned / NotAssigned are surfaced as `issue #N ...`.
        let lower = rest.to_lowercase();
        let (kind, detail) = if lower.contains("was modified since last read") {
            ("conflict", "re-read and retry")
        } else if lower.contains("is currently being worked on by") {
            ("assigned", "another session owns it")
        } else if lower.contains("is not assigned to session") {
            ("not owner", "run start first")
        } else if lower.contains("not found") {
            ("missing", "id does not exist")
        } else if lower.starts_with("#") && lower.contains("failed") {
            ("failed", "")
        } else {
            ("", "")
        };
        if !kind.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} {}",
                    styles.symbols.status_error,
                    truncate_to_width(first_line, max_width.saturating_sub(4))
                ),
                styles.error,
            )));
            if !detail.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("    {} {detail}", styles.symbols.arrow_right),
                    styles.muted,
                )));
            }
            // Show remaining lines muted (truncated).
            for extra in text.lines().skip(1).take(3) {
                lines.push(Line::from(Span::styled(
                    format!(
                        "    {}",
                        truncate_to_width(extra, max_width.saturating_sub(6))
                    ),
                    styles.muted,
                )));
            }
            return lines;
        }
    }

    // ── Success patterns ────────────────────────────────────────────────
    let success_style = styles.success;
    let neutral_style = styles.normal;
    let muted_style = styles.muted;

    if first_line.starts_with("created issue ") {
        // "created issue #12: Fix login bug"
        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                styles.symbols.status_success,
                truncate_to_width(first_line, max_width.saturating_sub(4))
            ),
            success_style,
        )));
        // Subsequent lines may be a follow-up notification (e.g., the
        // store emits nothing extra, but callers sometimes append).
        for extra in text.lines().skip(1).take(3) {
            lines.push(Line::from(Span::styled(
                format!(
                    "    {}",
                    truncate_to_width(extra, max_width.saturating_sub(6))
                ),
                muted_style,
            )));
        }
    } else if first_line.starts_with("closed issue ") {
        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                styles.symbols.status_success,
                truncate_to_width(first_line, max_width.saturating_sub(4))
            ),
            success_style,
        )));
    } else if first_line.starts_with("updated issue ") {
        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                styles.symbols.status_done,
                truncate_to_width(first_line, max_width.saturating_sub(4))
            ),
            neutral_style,
        )));
    } else if first_line.starts_with("released issue ") {
        // Released is a soft-yellow action — assignment ended.
        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                styles.symbols.radio_off,
                truncate_to_width(first_line, max_width.saturating_sub(4))
            ),
            styles.warning,
        )));
    } else if first_line.starts_with("linked session to issue ") {
        lines.push(Line::from(Span::styled(
            format!(
                "  + {}",
                truncate_to_width(first_line, max_width.saturating_sub(4))
            ),
            muted_style,
        )));
    } else if first_line.starts_with("assigned issue ") {
        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                styles.symbols.status_success,
                truncate_to_width(first_line, max_width.saturating_sub(4))
            ),
            success_style,
        )));
    } else if first_line == "no issues match the filter" {
        lines.push(Line::from(Span::styled(
            "  (no issues match the filter)".to_string(),
            muted_style,
        )));
    } else if first_line.starts_with("issue #") && first_line.contains(" failed: ") {
        // Slash command fallback: "issue #N start failed: <reason>"
        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                styles.symbols.status_error,
                truncate_to_width(first_line, max_width.saturating_sub(4))
            ),
            styles.error,
        )));
    } else {
        // Multi-line list result — color-code each entry.
        let total = text.lines().count();
        for (i, line) in text.lines().take(8).enumerate() {
            let display = truncate_to_width(line, max_width.saturating_sub(2));
            // Color by the status token in `[open]` / `[closed]`.
            let span = if display.contains("[closed]") {
                Span::styled(format!("  {display}"), muted_style)
            } else if display.contains("[open]") {
                Span::styled(format!("  {display}"), styles.normal)
            } else if display.contains("▣") {
                Span::styled(format!("  {display}"), styles.warning)
            } else {
                Span::styled(format!("  {display}"), muted_style)
            };
            lines.push(Line::from(span));
            // Hint at truncation on the last rendered line.
            if i == 7 && total > 8 {
                lines.push(Line::from(Span::styled(
                    format!("    … ({} more)", total - 8),
                    muted_style,
                )));
            }
        }
        if total == 0 {
            lines.push(Line::from(Span::styled(
                "  (empty result)".to_string(),
                muted_style,
            )));
        }
    }
    lines
}

/// Parse a priority string into a normalized display label.
/// Format a generic tool call (fallback for unknown tools).
pub fn format_generic_call(
    name: &str,
    args: &Value,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let icon = tool_icon(name, styles.symbols);
    let name_style = styles.accent.add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{icon} {name} "),
        name_style,
    )])];

    // Show first few args as key: value
    if let Some(obj) = args.as_object() {
        for (key, v) in obj.iter().take(3) {
            let val_str = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let prefix_len = 2 + UnicodeWidthStr::width(key.as_str()) + 2;
            let avail = max_width.saturating_sub(prefix_len);
            let display = truncate_to_width(&val_str, avail);
            lines.push(Line::from(vec![
                Span::styled(format!("  {}", key), styles.muted),
                Span::styled(": ", styles.muted),
                Span::styled(display, styles.normal),
            ]));
        }
    }

    lines
}

/// Format the `ask` tool call — renders each question prompt with its options
/// using omp-style radio (single) / checkbox (multi) markers, with a
/// "(Recommended)" suffix on the recommended option. Mirrors omp
/// `renderQuestionOptionLines`.
pub fn format_ask_call(args: &Value, max_width: usize, styles: &ThemeStyles) -> Vec<Line<'static>> {
    let questions = match args.get("questions").and_then(|v| v.as_array()) {
        Some(qs) if !qs.is_empty() => qs,
        _ => {
            // Missing or malformed arguments — degrade to a single header line.
            return vec![Line::from(Span::styled(
                format!("{} ask", tool_icon("ask", styles.symbols)),
                styles.accent.add_modifier(Modifier::BOLD),
            ))];
        }
    };

    let count = questions.len();
    let sym = styles.symbols;
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("{} Ask", sym.tool_ask),
            styles.accent.add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if count == 1 {
                format!("{}{} question", sym.sep_dot, count)
            } else {
                format!("{}{} questions", sym.sep_dot, count)
            },
            styles.muted,
        ),
    ])];

    for q in questions {
        let prompt = q
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("(no prompt)");
        let multi = q
            .get("multiSelect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let recommended = q
            .get("recommended")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        // Section tag for multi-question calls.
        if count > 1 {
            let id = q.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let mut meta: Vec<String> = Vec::new();
            if multi {
                meta.push("multi".to_string());
            }
            if let Some(opts) = q.get("options").and_then(|v| v.as_array()) {
                meta.push(format!("options:{}", opts.len()));
            }
            let meta_str = if meta.is_empty() {
                String::new()
            } else {
                format!("{}{}", sym.sep_dot, meta.join(sym.sep_dot))
            };
            lines.push(Line::from(vec![
                Span::styled(format!("[{}]", id), styles.muted),
                Span::styled(meta_str, styles.muted),
            ]));
        }

        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                sym.tool_ask,
                truncate_to_width(prompt, max_width.saturating_sub(4))
            ),
            styles.accent,
        )));

        if let Some(options) = q.get("options").and_then(|v| v.as_array()) {
            for (i, opt) in options.iter().enumerate() {
                let label = opt
                    .get("label")
                    .or_else(|| opt.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let is_rec = recommended == Some(i);
                let display = if is_rec {
                    format!("{}{}", label, RECOMMENDED_SUFFIX)
                } else {
                    label.to_string()
                };
                let marker = if multi {
                    sym.checkbox_off
                } else {
                    sym.radio_off
                };
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(format!("{} ", marker), styles.muted),
                    Span::styled(
                        truncate_to_width(&display, max_width.saturating_sub(6)),
                        if is_rec { styles.accent } else { styles.normal },
                    ),
                ]));
                // Description indented below (omp: ↳ desc)
                if let Some(desc) = opt.get("description").and_then(|v| v.as_str())
                    && !desc.trim().is_empty()
                {
                    lines.push(Line::from(vec![
                        Span::raw("      "),
                        Span::styled(format!("{} {}", sym.nav_expand, desc.trim()), styles.muted),
                    ]));
                }
            }
        }
    }

    lines
}

/// "(Recommended)" suffix — matches the overlay and omp `RECOMMENDED_SUFFIX`.
const RECOMMENDED_SUFFIX: &str = " (Recommended)";

/// Format the `ask` tool result as an omp-style "filled menu": every offered
/// option is re-shown with its selection marker filled in (radio filled /
/// checkbox checked), plus any custom free-text answer. Reconstructs the
/// option list from the call `arguments` (JSON) and matches selection from
/// the result text. Mirrors omp `renderAnswerOptionLines`.
///
/// `arguments` carries the original call JSON so tools like `ask` can
/// reconstruct a "filled menu" (all options + which were selected). Tools
/// that don't need it ignore the parameter.
pub fn format_ask_result(
    result: &str,
    arguments: Option<&str>,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let sym = styles.symbols;
    // Match the exact cancellation phrases the tool emits — not a loose
    // "cancel" substring, which would false-positive on ids/labels like
    // "cancel_policy: Full refund".
    let lower = result.to_lowercase();
    let is_cancelled = lower.starts_with("user cancelled")
        || lower.starts_with("question dismissed")
        || lower.starts_with("question cancelled");

    // Parse the call arguments to recover the question structure.
    let questions = arguments
        .and_then(|a| serde_json::from_str::<Value>(a).ok())
        .and_then(|v| v.get("questions").and_then(|q| q.as_array()).cloned())
        .unwrap_or_default();

    // Parse result lines into id → ParsedAnswer.
    let parsed = parse_ask_result(result);

    let header = if is_cancelled {
        Line::from(vec![
            Span::styled(
                format!("{} ", sym.status_warning),
                styles.warning.add_modifier(Modifier::BOLD),
            ),
            Span::styled("Ask", styles.warning.add_modifier(Modifier::BOLD)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                format!("{} ", sym.status_success),
                styles.success.add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} Ask", sym.tool_ask),
                styles.success.add_modifier(Modifier::BOLD),
            ),
        ])
    };
    let mut lines = vec![header];

    // If we couldn't recover the questions, fall back to raw result lines.
    if questions.is_empty() {
        for raw in result.lines() {
            if !raw.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", truncate_to_width(raw, max_width.saturating_sub(2))),
                    styles.muted,
                )));
            }
        }
        return lines;
    }

    for q in questions {
        let prompt = q.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        let id = q.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let multi = q
            .get("multiSelect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // `recommended` is not re-rendered in the answered view — the filled
        // marker is the signal. Options come from the call arguments.
        let options = q
            .get("options")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let answer = parsed.iter().find(|p| p.id == id);
        let selected: Vec<&str> = answer
            .map(|a| a.labels.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let selected_set: std::collections::HashSet<&str> = selected.iter().copied().collect();

        // Question prompt
        lines.push(Line::from(Span::styled(
            format!(
                "  {} {}",
                sym.tool_ask,
                truncate_to_width(prompt, max_width.saturating_sub(4))
            ),
            styles.accent,
        )));

        let has_selection = !selected.is_empty() || answer.is_some_and(|a| a.custom.is_some());

        if !has_selection {
            // No answer recorded (cancelled, or dismissed before reaching
            // this question). Show a clear warning marker instead of hollow
            // options — matches omp `renderAnswerOptionLines`.
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(format!("{} Cancelled", sym.status_warning), styles.warning),
            ]));
        } else {
            // Render every offered option with filled/hollow markers.
            for opt in options.iter() {
                let label = opt
                    .get("label")
                    .or_else(|| opt.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let is_sel = selected_set.contains(label);
                let (marker, mstyle, lstyle) = if is_sel {
                    let m = if multi { sym.checkbox_on } else { sym.radio_on };
                    (m, styles.success, styles.normal)
                } else {
                    let m = if multi {
                        sym.checkbox_off
                    } else {
                        sym.radio_off
                    };
                    (m, styles.muted, styles.muted)
                };
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(format!("{} ", marker), mstyle),
                    Span::styled(
                        truncate_to_width(label, max_width.saturating_sub(6)),
                        lstyle,
                    ),
                ]));
                // The filled marker already conveys the choice — no
                // "(Recommended)" suffix needed in the answered view.
            }
            // Custom free-text answer (allowOther).
            if let Some(custom) = answer.and_then(|a| a.custom.as_deref()) {
                for (ci, cline) in custom.lines().enumerate() {
                    if ci == 0 {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(format!("{} ", sym.status_success), styles.success),
                            Span::styled(
                                truncate_to_width(cline, max_width.saturating_sub(6)),
                                styles.normal,
                            ),
                        ]));
                    } else {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "      {}",
                                truncate_to_width(cline, max_width.saturating_sub(6))
                            ),
                            styles.normal,
                        )));
                    }
                }
            }
        }

        // Timeout footer.
        if answer.is_some_and(|a| a.timed_out) {
            lines.push(Line::from(Span::styled(
                "    auto-selected after timeout \u{2014} not a user choice".to_string(),
                styles.muted,
            )));
        }
    }

    lines
}

/// A parsed answer line from the ask tool result text.
struct ParsedAnswer {
    id: String,
    /// Selected option labels (single = one element, multi = many).
    labels: Vec<String>,
    /// Custom free-text input (allowOther), if any.
    custom: Option<String>,
    timed_out: bool,
}

/// Parse the ask result text into per-question answers.
///
/// Format produced by `ask::format_answers`:
/// - single: `id: label`
/// - multi:  `id: [a, b]`
/// - custom: `id: "text"`
/// - timeout suffix: ` (auto-selected after timeout)`
fn parse_ask_result(result: &str) -> Vec<ParsedAnswer> {
    let mut out = Vec::new();
    for raw in result.lines() {
        let timed_out = raw.contains("(auto-selected after timeout)");
        let body = raw.replace(" (auto-selected after timeout)", "");
        let Some((id, rest)) = body.split_once(": ") else {
            continue;
        };
        let rest = rest.trim();
        // custom: `"text"`
        let (labels, custom) =
            if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                (Vec::new(), Some(inner.to_string()))
            } else if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                // multi: [a, b]
                let labels = inner.split(", ").map(|s| s.trim().to_string()).collect();
                (labels, None)
            } else {
                (vec![rest.to_string()], None)
            };
        out.push(ParsedAnswer {
            id: id.trim().to_string(),
            labels,
            custom,
            timed_out,
        });
    }
    out
}
/// Format a tool call by tool name.
pub fn format_tool_call(
    name: &str,
    arguments: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let args = parse_tool_args(arguments);

    match name {
        "edit" => format_edit_call(&args, styles),
        "bash" => format_bash_call(&args, max_width, styles),
        "read" => format_read_call(&args, styles),
        "write" => format_write_call(&args, styles),
        "grep" | "find" | "ls" => format_search_call(name, &args, styles),
        "issue" => format_issue_call(&args, max_width, styles),
        "ask" => format_ask_call(&args, max_width, styles),
        _ => format_generic_call(name, &args, max_width, styles),
    }
}

// ── Result formatters ─────────────────────────────────────────────────────

/// Format an error result.
pub fn format_error_result(
    error: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for line in error.lines().take(4) {
        let display = truncate_to_width(line, max_width);
        lines.push(Line::from(Span::styled(
            format!("  {}", display),
            styles.error,
        )));
    }

    if error.lines().count() > 4 {
        lines.push(Line::from(Span::styled("  \u{2026}", styles.muted)));
    }

    lines
}

/// Format a unified diff result with colors.
pub fn format_diff_result(
    diff: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let total_lines = diff.lines().count();

    for raw_line in diff.lines().take(DIFF_PREVIEW_LINES) {
        let line = if raw_line.starts_with("@@") {
            // Hunk header
            Line::from(Span::styled(
                truncate_to_width(raw_line, max_width),
                styles.muted,
            ))
        } else if raw_line.starts_with('-') && !raw_line.starts_with("--") {
            // Removed line — red
            Line::from(Span::styled(
                format!(
                    " {}",
                    truncate_to_width(raw_line, max_width.saturating_sub(1))
                ),
                styles.error,
            ))
        } else if raw_line.starts_with('+') && !raw_line.starts_with("++") {
            // Added line — green
            Line::from(Span::styled(
                format!(
                    " {}",
                    truncate_to_width(raw_line, max_width.saturating_sub(1))
                ),
                styles.success,
            ))
        } else {
            // Context line
            Line::from(Span::styled(
                format!(
                    " {}",
                    truncate_to_width(raw_line, max_width.saturating_sub(1))
                ),
                styles.muted,
            ))
        };
        lines.push(line);
    }

    // Show diff stats if significant
    let (added, removed) = count_diff_stats(diff);
    if total_lines > DIFF_PREVIEW_LINES {
        lines.push(Line::from(Span::styled(
            format!(
                "  \u{2026} ({} more lines)",
                total_lines - DIFF_PREVIEW_LINES
            ),
            styles.muted,
        )));
    } else if added > 0 || removed > 0 {
        lines.push(Line::from(Span::styled(
            format!("  [+{} / -{}]", added, removed),
            styles.muted,
        )));
    }

    lines
}

/// Format a bash command result.
pub fn format_bash_result(
    result: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Show last N lines of output
    let all_lines: Vec<&str> = result.lines().collect();
    let _preview_lines = if all_lines.len() > RESULT_PREVIEW_LINES {
        // Show last N lines
        let start = all_lines.len() - RESULT_PREVIEW_LINES;
        for line in &all_lines[start..] {
            let display = truncate_to_width(line, max_width.saturating_sub(2));
            lines.push(Line::from(Span::styled(
                format!("  {}", display),
                styles.normal,
            )));
        }
        if start > 0 {
            lines.insert(
                0,
                Line::from(Span::styled(
                    format!("  … ({} earlier lines)", start),
                    styles.muted,
                )),
            );
        }
        all_lines.len()
    } else {
        for line in &all_lines {
            let display = truncate_to_width(line, max_width.saturating_sub(2));
            lines.push(Line::from(Span::styled(
                format!("  {}", display),
                styles.normal,
            )));
        }
        all_lines.len()
    };

    // Note: "Full output:" truncation info is intentionally NOT appended
    // as a separate line here. When the bash output is large, the tool
    // result already includes this text, and it will appear in the preview
    // naturally. Adding it separately caused a height measurement mismatch
    // (format_bash_result produced more lines than measure_result_height
    // expected), leading to content clipping in bordered tool boxes.

    lines
}

/// Format a read tool result (file content preview).
pub fn format_read_result(
    result: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let all_lines: Vec<&str> = result.lines().collect();
    let total = all_lines.len();

    // Show first N lines with line numbers
    for (i, line) in all_lines.iter().take(READ_PREVIEW_LINES).enumerate() {
        let line_num = i + 1;
        let display = truncate_to_width(line, max_width.saturating_sub(10));
        lines.push(Line::from(vec![
            Span::styled(format!("{:4} ", line_num), styles.muted),
            Span::styled(display, styles.normal),
        ]));
    }

    if total > READ_PREVIEW_LINES {
        let remaining = total - READ_PREVIEW_LINES;
        lines.push(Line::from(Span::styled(
            format!("  \u{2026} ({} more lines, {} total)", remaining, total),
            styles.muted,
        )));
    } else if total > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ({} lines)", total),
            styles.muted,
        )));
    }

    lines
}

/// Format a generic tool result (fallback).
pub fn format_generic_result(
    result: &str,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let all_lines: Vec<&str> = result.lines().collect();

    for line in all_lines.iter().take(GENERIC_PREVIEW_LINES) {
        let display = truncate_to_width(line, max_width.saturating_sub(2));
        lines.push(Line::from(Span::styled(
            format!("  {}", display),
            styles.normal,
        )));
    }

    if all_lines.len() > GENERIC_PREVIEW_LINES {
        lines.push(Line::from(Span::styled(
            format!(
                "  \u{2026} ({} more lines)",
                all_lines.len() - GENERIC_PREVIEW_LINES
            ),
            styles.muted,
        )));
    }

    lines
}

/// Format a tool result by tool name.
///
/// `arguments` carries the original call JSON so tools like `ask` can
/// reconstruct a "filled menu" (all options + which were selected). Tools
/// that don't need it ignore the parameter.
pub fn format_tool_result(
    name: &str,
    result: &str,
    is_error: bool,
    arguments: Option<&str>,
    max_width: usize,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    if is_error {
        return format_error_result(result, max_width, styles);
    }

    // Auto-detect diff if result contains diff markers
    if looks_like_diff(result) {
        return format_diff_result(result, max_width, styles);
    }

    match name {
        "edit" => format_diff_result(result, max_width, styles),
        "bash" => format_bash_result(result, max_width, styles),
        "read" => format_read_result(result, max_width, styles),
        "issue" => format_issue_result(result, max_width, styles),
        "ask" => format_ask_result(result, arguments, max_width, styles),
        _ => format_generic_result(result, max_width, styles),
    }
}

// ── Height calculation ─────────────────────────────────────────────────────

/// Calculate the rendered height for a tool call.
pub fn measure_call_height(name: &str, arguments: &str, max_width: usize) -> u16 {
    let args = parse_tool_args(arguments);

    match name {
        "edit" => format_edit_call(&args, &ThemeStyles::default()).len() as u16,
        "bash" => format_bash_call(&args, max_width, &ThemeStyles::default()).len() as u16,
        "read" => 1, // Always 1 line for read
        "write" => 1,
        "grep" | "find" | "ls" => 1,
        "issue" => {
            // Header + (title + optional tags) for create, or
            // header + #id + (optional changed-fields) for update,
            // or just header + #id for the rest.
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let base = 1u16;
            match action {
                "create" => {
                    base + 1
                        + if args.get("priority").is_some() || args.get("labels").is_some() {
                            1
                        } else {
                            0
                        }
                }
                "list" => base + 1,
                "update" => {
                    let mut extra = 1; // #id line
                    let mut changed = 0u16;
                    if args.get("title").is_some() {
                        changed += 1;
                    }
                    if args.get("body").is_some() {
                        changed += 1;
                    }
                    if args.get("priority").is_some() {
                        changed += 1;
                    }
                    if args.get("status").is_some() {
                        changed += 1;
                    }
                    if args.get("labels").is_some() {
                        changed += 1;
                    }
                    if changed > 0 {
                        extra += 1;
                    }
                    base + extra
                }
                _ => base + 1,
            }
        }
        "ask" => format_ask_call(&args, max_width, &ThemeStyles::default()).len() as u16,
        _ => {
            // Generic: 1 header + up to 3 args
            let args_count = args.as_object().map(|o| o.len().min(3)).unwrap_or(0);
            1 + args_count as u16
        }
    }
}

/// Calculate the rendered height for a tool result.
pub fn measure_result_height(name: &str, result: &str, is_error: bool) -> u16 {
    if is_error {
        let total = result.lines().count();
        let shown = total.min(4);
        return shown as u16 + if total > 4 { 1 } else { 0 };
    }

    if looks_like_diff(result) {
        let total = result.lines().count();
        let shown = total.min(DIFF_PREVIEW_LINES);
        let extra = if total > DIFF_PREVIEW_LINES
            || count_diff_stats(result).0 > 0
            || count_diff_stats(result).1 > 0
        {
            1
        } else {
            0
        };
        return shown as u16 + extra;
    }

    match name {
        "edit" => {
            let total = result.lines().count();
            let shown = total.min(DIFF_PREVIEW_LINES);
            let has_more = total > DIFF_PREVIEW_LINES;
            let has_stats =
                !has_more && (count_diff_stats(result).0 > 0 || count_diff_stats(result).1 > 0);
            let extra = if has_more || has_stats { 1 } else { 0 };
            shown as u16 + extra
        }
        "bash" => {
            let total = result.lines().count();
            let shown = total.min(RESULT_PREVIEW_LINES);
            let extra = if total > RESULT_PREVIEW_LINES { 1 } else { 0 };
            shown as u16 + extra
        }
        "read" => {
            let total = result.lines().count();
            let shown = total.min(READ_PREVIEW_LINES);
            let extra: u16 = 1; // Always show count
            shown as u16 + extra
        }
        "issue" => {
            // We render at most 9 lines (header + up to 8 list entries, or
            // 1 success line + 0–3 follow-ups, or 1 error line + 0–3 details).
            let first = result.lines().next().unwrap_or("");
            let lower = first.to_lowercase();
            let is_error = first.starts_with("issue ")
                && (lower.contains("was modified since last read")
                    || lower.contains("is currently being worked on by")
                    || lower.contains("is not assigned to session")
                    || lower.contains("not found"));
            if is_error {
                1 + result.lines().skip(1).take(3).count() as u16
            } else if first.starts_with("created issue ")
                || first.starts_with("closed issue ")
                || first.starts_with("updated issue ")
                || first.starts_with("released issue ")
                || first.starts_with("linked session to issue ")
                || first.starts_with("assigned issue ")
                || first == "no issues match the filter"
                || (first.starts_with("issue #") && first.contains(" failed: "))
            {
                // Header + up to 3 follow-up lines.
                (1 + result.lines().skip(1).take(3).count() as u16).min(4)
            } else {
                // Multi-line list — cap at 8 + 1 trailing hint.
                let total = result.lines().count();
                let shown = total.min(8);
                let extra = if total > 8 { 1 } else { 0 };
                shown as u16 + extra
            }
        }
        "ask" => 1 + result.lines().count() as u16,
        _ => {
            let total = result.lines().count();
            let shown = total.min(GENERIC_PREVIEW_LINES);
            let extra = if total > GENERIC_PREVIEW_LINES { 1 } else { 0 };
            shown as u16 + extra
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorten_path() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        let path = format!("{}/src/main.rs", home);
        assert_eq!(shorten_path(&path), "~/src/main.rs");
    }

    #[test]
    fn test_parse_args() {
        // Use actual home directory so shorten_path works
        let home = dirs::home_dir().unwrap();
        let path = format!("{}/file.rs", home.to_string_lossy());
        let args = format!(r#"{{"path":"{}","offset":10,"limit":50}}"#, path);
        let parsed = parse_tool_args(&args);
        assert_eq!(get_path(&parsed), Some("~/file.rs".to_string()));
        assert_eq!(get_int(&parsed, "offset"), Some(10));
        assert_eq!(get_int(&parsed, "limit"), Some(50));
    }

    #[test]
    fn test_looks_like_diff() {
        assert!(looks_like_diff("@@ -10,3 +10,4 @@\n-old\n+new"));
        assert!(looks_like_diff("-removed\n+added"));
        assert!(!looks_like_diff("hello world\njust text"));
    }

    #[test]
    fn test_count_diff_stats() {
        let diff = "-removed\n-old\n+added\n+new";
        let (added, removed) = count_diff_stats(diff);
        assert_eq!(added, 2);
        assert_eq!(removed, 2);
    }

    #[test]
    fn test_truncate_to_width() {
        let text = "hello world";
        assert_eq!(truncate_to_width(text, 100), "hello world");
        // For max_width=5: "hello" (5 chars) overflows, so truncate to 4 + ellipsis
        // This matches the behavior of the original truncate_str in chat.rs
        assert_eq!(truncate_to_width(text, 5), "hell…");
        assert_eq!(truncate_to_width(text, 0), "");
    }

    #[test]
    fn test_format_edit_call() {
        let args = serde_json::json!({
            "path": "/home/user/src/main.rs",
            "edits": [{"oldText": "foo", "newText": "bar"}]
        });
        let lines = format_edit_call(&args, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("edit"));
        assert!(lines[0].to_string().contains("main.rs"));
    }

    #[test]
    fn test_format_bash_call() {
        let args = serde_json::json!({
            "command": "cargo build --release",
            "timeout": 120
        });
        let lines = format_bash_call(&args, 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("$ cargo build"));
    }

    #[test]
    fn test_format_read_call() {
        let args = serde_json::json!({
            "path": "/home/user/src/main.rs",
            "offset": 10,
            "limit": 50
        });
        let lines = format_read_call(&args, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("read"));
        assert!(lines[0].to_string().contains("main.rs:10-59"));
    }

    #[test]
    fn test_format_diff_result() {
        let diff = "@@ -10,3 +10,4 @@\n-old line\n context\n+new line\n+extra line";
        let lines = format_diff_result(diff, 80, &ThemeStyles::default());
        // Should have hunk header + lines + stats
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_format_bash_result() {
        let output = "Compiling mycrate v0.1.0\nFinished release [optimized]";
        let lines = format_bash_result(output, 80, &ThemeStyles::default());
        assert!(!lines.is_empty());
        assert!(lines[0].to_string().contains("Compiling"));
    }

    #[test]
    fn test_format_read_result() {
        let content = "use std::io;\n\nfn main() {\n    println!(\"hello\");\n}\n";
        let lines = format_read_result(content, 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("1"));
        assert!(lines[0].to_string().contains("use std"));
    }

    #[test]
    fn test_format_generic_call() {
        let args = serde_json::json!({
            "param1": "value1",
            "param2": "value2"
        });
        let lines = format_generic_call("my_tool", &args, 80, &ThemeStyles::default());
        assert!(!lines.is_empty());
        assert!(lines[0].to_string().contains("my_tool"));
    }

    #[test]
    fn test_has_exit_status() {
        assert!(has_exit_status("Command exited with code 1"));
        assert!(has_exit_status("Command timed out after 120 seconds"));
        assert!(!has_exit_status("Everything is fine"));
    }

    #[test]
    fn test_format_issue_call_create() {
        let args = serde_json::json!({
            "action": "create",
            "title": "Fix login bug",
            "priority": "high",
            "labels": ["bug", "auth"],
        });
        let lines = format_issue_call(&args, 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("issue"));
        assert!(lines[0].to_string().contains("create"));
        // Title appears quoted on line 2
        assert!(
            lines
                .iter()
                .any(|l| l.to_string().contains("Fix login bug"))
        );
        // Priority chip present
        assert!(lines.iter().any(|l| l.to_string().contains("[high]")));
        // Labels chip present
        assert!(lines.iter().any(|l| l.to_string().contains("bug,auth")));
    }

    #[test]
    fn test_format_issue_call_start() {
        let args = serde_json::json!({"action": "start", "id": 12});
        let lines = format_issue_call(&args, 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("start"));
        assert!(lines[1].to_string().contains("#12"));
    }

    #[test]
    fn test_format_issue_call_list_with_filters() {
        let args = serde_json::json!({
            "action": "list",
            "status": "open",
            "priority": "high",
            "label": "auth",
            "text": "login",
        });
        let lines = format_issue_call(&args, 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("list"));
        let combined: String = lines.iter().map(|l| l.to_string()).collect();
        assert!(combined.contains("status=open"));
        assert!(combined.contains("priority=high"));
        assert!(combined.contains("label=auth"));
        assert!(combined.contains("text="));
    }

    #[test]
    fn test_format_issue_result_success() {
        let result = "created issue #12: Fix login bug";
        let lines = format_issue_result(result, 80, &ThemeStyles::default());
        let sym = ThemeStyles::default().symbols.status_success;
        assert!(lines[0].to_string().contains(sym));
        assert!(lines[0].to_string().contains("created issue #12"));
    }

    #[test]
    fn test_format_issue_result_closed() {
        let lines = format_issue_result(
            "closed issue #7: Outdated task",
            80,
            &ThemeStyles::default(),
        );
        assert!(
            lines[0]
                .to_string()
                .contains(ThemeStyles::default().symbols.status_success)
        );
        assert!(lines[0].to_string().contains("closed"));
    }

    #[test]
    fn test_format_issue_result_released_warning_color() {
        let lines = format_issue_result("released issue #3", 80, &ThemeStyles::default());
        // Released uses warning style; we only assert semantic content here.
        assert!(lines[0].to_string().contains("released"));
    }

    #[test]
    fn test_format_issue_result_empty_list() {
        let lines = format_issue_result("no issues match the filter", 80, &ThemeStyles::default());
        assert!(lines[0].to_string().contains("no issues"));
    }

    #[test]
    fn test_format_issue_result_conflict() {
        let lines = format_issue_result(
            "issue #12 was modified since last read; re-read and retry",
            80,
            &ThemeStyles::default(),
        );
        // Conflict marker must appear.
        assert!(
            lines[0]
                .to_string()
                .contains(ThemeStyles::default().symbols.status_error)
        );
        assert!(
            lines.iter().any(|l| l.to_string().contains("Conflict"))
                || lines.iter().any(|l| l.to_string().contains("re-read"))
        );
    }

    #[test]
    fn test_format_issue_result_assigned_error() {
        let lines = format_issue_result(
            "issue #12 is currently being worked on by session tui",
            80,
            &ThemeStyles::default(),
        );
        assert!(
            lines[0]
                .to_string()
                .contains(ThemeStyles::default().symbols.status_error)
        );
    }

    #[test]
    fn test_format_issue_result_list_multiline_color() {
        let result = "\
#1    [open]    medium     Fix login bug
#2    [open]    high   ▣  Refactor auth
#3    [closed]  low       Old task";
        let lines = format_issue_result(result, 80, &ThemeStyles::default());
        // 3 entries + no truncation hint.
        assert!(lines.len() >= 3);
        assert!(lines.iter().any(|l| l.to_string().contains("#1")));
        assert!(lines.iter().any(|l| l.to_string().contains("#3")));
    }

    #[test]
    fn test_format_issue_call_unknown_action() {
        let args = serde_json::json!({"action": "frobnicate"});
        let lines = format_issue_call(&args, 80, &ThemeStyles::default());
        assert!(
            lines
                .iter()
                .any(|l| l.to_string().contains("unknown action"))
        );
    }

    #[test]
    fn test_measure_issue_call_create() {
        // 1 header + 1 title + 1 tags = 3
        let args = r#"{"action":"create","title":"X","priority":"high"}"#;
        assert!(measure_call_height("issue", args, 80) >= 3);
    }

    #[test]
    fn test_measure_issue_call_id_only() {
        let args = r#"{"action":"start","id":1}"#;
        // 1 header + 1 id = 2
        assert!(measure_call_height("issue", args, 80) >= 2);
    }

    #[test]
    fn test_measure_issue_result_success_short() {
        let result = "created issue #12: Title";
        assert!(measure_result_height("issue", result, false) >= 1);
    }

    #[test]
    fn test_measure_issue_result_long_list() {
        let result = (1..=20)
            .map(|i| format!("#{i:<4} [open]    medium     Issue {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // Capped at 8 + 1 hint = 9 lines max.
        let h = measure_result_height("issue", &result, false);
        assert!(h <= 10, "expected height ≤10, got {h}");
    }
}
