/// AST-grep tool — structural code search using `sg` (ast-grep CLI).
use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;

/// Default max results when caller does not specify `limit`.
const DEFAULT_LIMIT: usize = 50;

/// AstGrepTool — wraps the `sg` (ast-grep) CLI for structural code search.
pub struct AstGrepTool {
    root_dir: Option<PathBuf>,
}

impl AstGrepTool {
    /// Create with no explicit root (uses ToolContext.root() at runtime).
    pub fn new() -> Self {
        Self { root_dir: None }
    }

    /// Create with a specific working directory (overrides ToolContext).
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            root_dir: Some(cwd),
        }
    }

    /// Resolve a `path` argument relative to the tool's effective root.
    /// Falls back to the root directory when `path` is empty.
    fn resolve_search_path(&self, path: &str, ctx_root: &Path) -> PathBuf {
        if path.is_empty() {
            ctx_root.to_path_buf()
        } else {
            let candidate = PathBuf::from(path);
            if candidate.is_absolute() {
                candidate
            } else {
                ctx_root.join(candidate)
            }
        }
    }
}

impl Default for AstGrepTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Run `sg run -p <pattern> --json <path>` and return the parsed match
/// objects as a `Vec<Value>`.
///
/// `run` is the default ast-grep subcommand and is required when other
/// flags follow; bare `sg -p …` is a clap usage error. We pass bare
/// `--json` and let [`parse_sg_output`] deal with whatever style
/// ast-grep emits — see that function's doc for the three shapes handled.
///
/// Returns `Err("`sg` is not installed…")` when the binary is missing,
/// `Err(...)` when the process reports a real failure on stderr, and
/// `Ok(matches)` on success (including the empty-Vec "no matches" case —
/// ast-grep exits 1 with empty stdout when the pattern is well-formed
/// but produces no hits).
async fn run_sg(pattern: &str, target: &Path) -> Result<Vec<Value>, String> {
    let mut child = match Command::new("sg")
        .arg("run")
        .arg("-p")
        .arg(pattern)
        .arg("--json")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                "`sg` (ast-grep CLI) is not installed or not on PATH. Install it from https://ast-grep.github.io/ to use the ast_grep tool."
                    .to_string(),
            );
        }
        Err(e) => return Err(format!("Failed to invoke `sg`: {e}")),
    };

    // SAFETY: the command was spawned with `Stdio::piped()` for stdout/stderr
    // and the spawn succeeded (we returned early on error), so both `take()`
    // calls cannot return None.
    #[allow(clippy::expect_used)]
    let mut stdout = child.stdout.take().expect("piped stdout");
    #[allow(clippy::expect_used)]
    let mut stderr = child.stderr.take().expect("piped stderr");

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let (stdout_res, stderr_res) = tokio::join!(
        stdout.read_to_end(&mut stdout_buf),
        stderr.read_to_end(&mut stderr_buf)
    );
    stdout_res.map_err(|e| format!("Failed reading `sg` stdout: {e}"))?;
    stderr_res.map_err(|e| format!("Failed reading `sg` stderr: {e}"))?;

    let status = child
        .wait()
        .await
        .map_err(|e| format!("Failed waiting on `sg`: {e}"))?;

    // Parse stdout in a format-agnostic way: ast-grep supports three
    // `--json` modes (`pretty`, `stream`, `compact`) and a future version
    // could change the default. Try the array forms first (pretty emits
    // a multi-line `[ {…}, {…} ]`, compact emits it on one line), then
    // fall back to NDJSON (stream emits one `{…}` per line).
    let matches = parse_sg_output(&stdout_buf).ok_or_else(|| {
        "Failed to parse `sg` JSON output: no array or stream objects found".to_string()
    })?;

    // ast-grep exits non-zero on no matches (stdout empty) or on real
    // failures (stderr populated). Distinguish by stderr content.
    if !status.success() {
        let stderr_text = String::from_utf8_lossy(&stderr_buf).trim().to_string();
        if !stderr_text.is_empty() {
            return Err(format!("`sg` failed: {stderr_text}"));
        }
    }

    Ok(matches)
}

/// Parse the bytes emitted by `sg --json` regardless of output style.
///
/// Three documented styles:
/// - `pretty` (the default): a multi-line JSON `[{…}, {…}, …]` array.
/// - `compact`: a single-line JSON `[{…},{…}]` array.
/// - `stream`: one JSON object `{…}` per line (NDJSON).
///
/// This helper accepts all three:
/// 1. If the entire buffer parses as a JSON `Value::Array`, return it.
/// 2. If it parses as a single `Value::Object`, wrap and return it.
/// 3. Otherwise, treat each non-blank line as a separate JSON object and
///    return the collected Vec (NDJSON).
/// 4. If none of the above yield any matches, return `None` so the caller
///    can decide between "no output" (success, zero matches) and a parse
///    error.
fn parse_sg_output(buf: &[u8]) -> Option<Vec<Value>> {
    let trimmed = buf.iter().any(|b| !b.is_ascii_whitespace());
    if !trimmed {
        return Some(Vec::new());
    }

    // Try the whole-buffer shapes first.
    if let Ok(v) = serde_json::from_slice::<Value>(buf) {
        match v {
            Value::Array(arr) => return Some(arr),
            Value::Object(_) => return Some(vec![v]),
            _ => {}
        }
    }

    // Fall back to NDJSON (one match object per line).
    let mut matches = Vec::new();
    let mut parsed_any = false;
    for line in buf.split(|b| *b == b'\n') {
        let has_content = line.iter().any(|b| !b.is_ascii_whitespace());
        if !has_content {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(v) => {
                parsed_any = true;
                match v {
                    Value::Array(arr) => matches.extend(arr),
                    // `stream` style documents one OBJECT per line, but
                    // tolerate an unbracketed single object too.
                    Value::Object(_) => matches.push(v),
                    _ => {}
                }
            }
            Err(_) => continue,
        }
    }

    if parsed_any { Some(matches) } else { None }
}

/// Format matches grouped by directory + file with line numbers.
///
/// Output shape (one section per file):
/// ```text
/// <relative-path>
///   <line>:<col>-<end_col>: <trimmed text>
///   ...
/// ```
/// Returns `(formatted_output, returned_count)`. Callers apply pagination
/// BEFORE calling this — inner truncation logic was dead.
fn format_matches(matches: &[Value], root: &Path) -> (String, usize) {
    use std::collections::BTreeMap;

    if matches.is_empty() {
        return ("No matches found.".to_string(), 0);
    }

    // file -> Vec<(line, col, text)>
    let mut by_file: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();

    for m in matches {
        let file = m
            .get("file")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("<unknown>"));

        // Range shape: { "start": {"line": N, "column": N}, "end": {...} }
        // Older ast-grep emits `{ begin, end }` instead. Handle both.
        let (line, col) = extract_position(m).unwrap_or((0, 0));

        let text = m
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();

        // Trim trailing whitespace but keep indentation for readability.
        let trimmed = text.lines().next().unwrap_or("").trim_end().to_string();

        by_file.entry(file).or_default().push((line, col, trimmed));
    }

    let returned = matches.len();
    let mut out = String::new();
    out.push_str(&format!("Found {returned} match(es):\n"));

    for (file, lines) in &by_file {
        let display = file.strip_prefix(root).unwrap_or(file.as_path());
        let display = display.to_string_lossy();
        out.push('\n');
        out.push_str(&format!("{display}\n"));
        for (line, col, text) in lines {
            if *col > 0 {
                out.push_str(&format!("  {line}:{col}: {text}\n"));
            } else {
                out.push_str(&format!("  {line}: {text}\n"));
            }
        }
    }

    (out, returned)
}

/// Extract (line, column) from a `sg --json` match object, tolerating both
/// the new `{ range: { start, end } }` shape and the older `{ begin, end }`
/// shape. Lines and columns are 0-indexed in `sg` output; we display
/// 1-indexed line numbers (line + 1) but pass columns through as-is.
fn extract_position(m: &Value) -> Option<(usize, usize)> {
    if let Some(range) = m.get("range").and_then(Value::as_object) {
        let start = range.get("start").and_then(Value::as_object)?;
        let line = start.get("line").and_then(Value::as_u64)? as usize;
        let col = start
            .get("column")
            .or_else(|| start.get("col"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        return Some((line + 1, col + 1));
    }

    if let Some(begin) = m.get("begin").and_then(Value::as_u64) {
        return Some((begin as usize + 1, 1));
    }

    None
}

#[async_trait]
impl AgentTool for AstGrepTool {
    fn name(&self) -> &str {
        "ast_grep"
    }

    fn label(&self) -> &str {
        "AST Grep"
    }

    fn description(&self) -> &str {
        "Structural code search using ast-grep. Pattern uses ast-grep pattern syntax (e.g. 'fn $NAME($$$ARGS) { $$$BODY }'). Runs `sg run -p <pattern> --json <path>` and groups results by file with line numbers. Requires the `sg` (ast-grep) CLI to be installed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "AST pattern in ast-grep syntax (e.g. 'fn $NAME($$$ARGS) { $$$BODY }'). Metavariables use uppercase `$NAME`; zero-or-more use `$$$NAME`."
                },
                "path": {
                    "type": "string",
                    "description": "File, directory, or glob to search. Defaults to the workspace root."
                },
                "skip": {
                    "type": "integer",
                    "description": "Number of results to skip (for pagination).",
                    "minimum": 0,
                    "default": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return.",
                    "minimum": 1,
                    "default": 50
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // ── 1. Validate pattern ───────────────────────────────────────
        let pattern = params
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing required parameter: pattern".to_string())?
            .trim();

        if pattern.is_empty() {
            return Ok(AgentToolResult::error(
                "Invalid pattern: must be a non-empty string",
            ));
        }

        // ── 2. Resolve search scope ───────────────────────────────────
        let path_arg = params.get("path").and_then(Value::as_str).unwrap_or("");

        let skip = params.get("skip").and_then(Value::as_u64).unwrap_or(0) as usize;

        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_LIMIT as u64) as usize;
        let limit = limit.max(1);

        let root = self.root_dir.as_deref().unwrap_or_else(|| ctx.root());
        let search_path = self.resolve_search_path(path_arg, root);

        // ── 3. Run `sg` and parse JSON stream ─────────────────────────
        let all_matches = match run_sg(pattern, &search_path).await {
            Ok(v) => v,
            Err(msg) if msg.starts_with("`sg` is not installed") => {
                return Ok(AgentToolResult::error(msg));
            }
            Err(msg) => return Ok(AgentToolResult::error(format!("ast_grep failed: {msg}"))),
        };

        let total = all_matches.len();

        // Apply pagination: skip the first `skip` items, then keep at
        // most `limit`. Truncated == "there were more matches the caller
        // didn't see" — i.e. `total > skip + returned`. We deliberately
        // avoid the `skip > 0` heuristic here: paginating past the first
        // page is normal, not a truncation signal.
        let paged: Vec<Value> = all_matches.into_iter().skip(skip).take(limit).collect();
        let returned = paged.len();
        let truncated = total > skip + returned;

        // ── 4. Format grouped results ────────────────────────────────
        let (body, _returned_fmt) = format_matches(&paged, root);
        let mut result = AgentToolResult::success(body);
        result.metadata = Some(json!({
            "total_matches": total,
            "returned": returned,
            "skipped": skip,
            "limit": limit,
            "truncated": truncated,
            "pattern": pattern,
            "search_path": search_path.to_string_lossy(),
        }));

        Ok(result)
    }
}
