//! AST-edit tool — structural code rewriting via the `sg` (ast-grep) CLI.
//!
//! Like the `ast_grep` search tool but applies pattern-driven rewrites.
//! Accepts an array of `{ pat, out }` operations applied against a list of
//! files / directories / globs.
//!
//! ## Empirical CLI behavior (verified against ast-grep 0.45.0)
//!
//! - `sg` does **not** expand shell globs in positional path args:
//!   `sg -p P --json 'src/**/*.rs'` errors with `No such file or directory`.
//!   This tool expands globs via the `glob` crate before invoking `sg`.
//! - `-U` (`--update-all`) is **silently ignored** when `--json` is set: the
//!   process exits 0, prints JSON, and the file is untouched. We therefore
//!   drop `--json` for the apply pass.
//! - With `-U` and no `--json`, `sg` writes changes in place AND prints
//!   `Applied N changes` on **stderr** — so we get the replacement count
//!   without a second dry-run pass.
//! - `--json=stream` emits one JSON object per match per line on stdout,
//!   making it cheap to count and group by file.
//! - `sg` accepts any number of positional paths (directories + concrete
//!   files + globs-expanded files) in a single invocation, so we issue one
//!   process per (op × path-chunk) and never loop over individual paths.
//!
//! ## Modes
//!
//! - `dry_run=true` (default) — previews via `sg -p P -r R --json=stream`.
//!   No files are modified; counts come from stdout line count.
//! - `dry_run=false` — applies via `sg -p P -r R -U`. Counts come from the
//!   `Applied N changes` line on stderr.
use super::path_security::PathGuard;
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;

/// Cap on how many paths we'll pass to a single `sg` invocation.
/// `sg` walks directories internally, so this only counts the input list.
const MAX_PATHS_PER_INVOCATION: usize = 512;

/// Cap on stdout we'll buffer for a dry-run preview (bytes). ast-grep's
/// `--json=stream` output is one line per match, so 8 MiB is generous.
const DRY_RUN_STDOUT_CAP: usize = 8 * 1024 * 1024;

/// One pattern → replacement operation supplied by the caller.
#[derive(Debug, Clone)]
struct RewriteOp {
    pat: String,
    out: String,
}

/// AstEditTool — wraps the `sg` (ast-grep) CLI for structural rewriting.
pub struct AstEditTool {
    root_dir: Option<PathBuf>,
}

impl AstEditTool {
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

    /// Resolve a single user-supplied path string against the tool root.
    /// Absolute paths pass through; relative paths join onto the root.
    fn resolve_one(raw: &str, root: &Path) -> PathBuf {
        let candidate = PathBuf::from(raw);
        if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        }
    }

    /// True if the path string contains shell-glob wildcard characters.
    fn looks_like_glob(raw: &str) -> bool {
        raw.contains('*') || raw.contains('?') || raw.contains('[')
    }

    /// Expand a list of user paths (files, dirs, or globs) into a flat list
    /// of `sg`-ready positional args: a mix of concrete file paths and
    /// directories (which `sg` walks itself).
    ///
    /// `sg` does not expand globs in positional args (verified: it errors
    /// with `No such file or directory`), so we expand them here via the
    /// `glob` crate. Directories pass through unchanged.
    fn expand_paths(raw_paths: &[String], root: &Path) -> Result<Vec<PathBuf>, ToolError> {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for raw in raw_paths {
            if Self::looks_like_glob(raw) {
                let candidate = Self::resolve_one(raw, root);
                let pattern_str = candidate.to_string_lossy().into_owned();
                let entries = glob::glob(&pattern_str)
                    .map_err(|e| format!("Invalid glob pattern '{}': {}", raw, e))?;
                let mut matched_any = false;
                for entry in entries {
                    let p = entry.map_err(|e| format!("Glob error for '{}': {}", raw, e))?;
                    matched_any = true;
                    // `sg` walks directories, so we can pass them through
                    // too — but only when the glob explicitly targets a
                    // directory (rare). For file globs, filter to files.
                    if p.is_dir() {
                        if seen.insert(p.clone()) {
                            out.push(p);
                        }
                    } else if p.is_file() && seen.insert(p.clone()) {
                        out.push(p);
                    }
                }
                if !matched_any {
                    return Err(format!("Glob '{}' matched no files", raw));
                }
            } else {
                let candidate = Self::resolve_one(raw, root);
                if !candidate.exists() {
                    return Err(format!("Path not found: {}", raw));
                }
                if seen.insert(candidate.clone()) {
                    out.push(candidate);
                }
            }
        }

        if out.is_empty() {
            return Err("No files matched the supplied paths/globs".to_string());
        }

        Ok(out)
    }
}

impl Default for AstEditTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse `ops` from the input `Value`.
fn parse_ops(params: &Value) -> Result<Vec<RewriteOp>, ToolError> {
    let arr = params
        .get("ops")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required parameter: ops (must be an array)".to_string())?;

    if arr.is_empty() {
        return Err("Parameter 'ops' must contain at least one { pat, out } entry".to_string());
    }

    let mut ops = Vec::with_capacity(arr.len());
    for (i, op) in arr.iter().enumerate() {
        let pat = op
            .get("pat")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("ops[{}]: missing or non-string 'pat'", i))?
            .to_string();
        let out = op
            .get("out")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("ops[{}]: missing or non-string 'out'", i))?
            .to_string();

        if pat.trim().is_empty() {
            return Err(format!("ops[{}]: 'pat' must be a non-empty string", i));
        }

        ops.push(RewriteOp { pat, out });
    }

    Ok(ops)
}

/// Parse `paths` from the input `Value`.
fn parse_paths(params: &Value) -> Result<Vec<String>, ToolError> {
    let arr = params
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required parameter: paths (must be an array)".to_string())?;

    if arr.is_empty() {
        return Err("Parameter 'paths' must contain at least one path".to_string());
    }

    let mut paths = Vec::with_capacity(arr.len());
    for (i, p) in arr.iter().enumerate() {
        let s = p
            .as_str()
            .ok_or_else(|| format!("paths[{}]: must be a string", i))?;
        paths.push(s.to_string());
    }

    Ok(paths)
}

/// Spawn `sg` for a single (op, path-chunk) and return its `(status, stdout, stderr)`.
///
/// For dry-run previews we use `--json=stream` (one JSON object per match
/// per line — cheap to count, easy to group by file). For real rewrites we
/// drop `--json` and add `-U` because ast-grep silently ignores `-U` when
/// `--json` is set; `Applied N changes` then lands on stderr.
async fn run_sg_for_op(
    op: &RewriteOp,
    paths: &[PathBuf],
    dry_run: bool,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), String> {
    let mut cmd = Command::new("sg");
    cmd.arg("-p").arg(&op.pat).arg("-r").arg(&op.out);

    if dry_run {
        // --json=stream: one match per line, easy to count.
        cmd.arg("--json=stream");
    } else {
        // Real rewrite. ast-grep ignores -U when --json is set, so we MUST
        // omit --json here. The `Applied N changes` summary lands on stderr.
        cmd.arg("-U");
    }

    for p in paths {
        cmd.arg(p);
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(
                "`sg` (ast-grep CLI) is not installed or not on PATH. Install it from https://ast-grep.github.io/ to use the ast_edit tool."
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

    if dry_run {
        // Cap dry-run stdout so a runaway codebase doesn't OOM the agent.
        let mut limited = stdout.take(DRY_RUN_STDOUT_CAP as u64);
        let (s_res, e_res) = tokio::join!(
            AsyncReadExt::read_to_end(&mut limited, &mut stdout_buf),
            stderr.read_to_end(&mut stderr_buf)
        );
        s_res.map_err(|e| format!("Failed reading `sg` stdout: {e}"))?;
        e_res.map_err(|e| format!("Failed reading `sg` stderr: {e}"))?;
    } else {
        let (s_res, e_res) = tokio::join!(
            stdout.read_to_end(&mut stdout_buf),
            stderr.read_to_end(&mut stderr_buf)
        );
        s_res.map_err(|e| format!("Failed reading `sg` stdout: {e}"))?;
        e_res.map_err(|e| format!("Failed reading `sg` stderr: {e}"))?;
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("Failed waiting on `sg`: {e}"))?;

    Ok((status, stdout_buf, stderr_buf))
}

/// Count and group dry-run matches from `sg --json=stream` output.
///
/// Each non-empty line is one match JSON object. We only need a count and
/// per-file breakdown, not the full payload, so malformed lines are
/// silently skipped (counted toward total if they at least parsed as JSON).
fn summarise_dry_run(stdout: &[u8]) -> (usize, std::collections::BTreeMap<PathBuf, usize>) {
    let mut total = 0usize;
    let mut by_file: std::collections::BTreeMap<PathBuf, usize> = std::collections::BTreeMap::new();

    for line in stdout.split(|b| *b == b'\n') {
        let trimmed: Vec<u8> = line
            .iter()
            .copied()
            .skip_while(|b| b.is_ascii_whitespace())
            .take_while(|b| !b.is_ascii_whitespace())
            .collect();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&trimmed) {
            if let Some(file) = v.get("file").and_then(Value::as_str) {
                *by_file.entry(PathBuf::from(file)).or_insert(0) += 1;
            }
            total += 1;
        }
    }

    (total, by_file)
}

/// Parse `Applied N changes` out of `sg -U` stderr.
fn parse_applied_count(stderr: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(stderr);
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Applied ") {
            let num = rest.split_whitespace().next()?;
            return num.parse::<usize>().ok();
        }
    }
    None
}

/// Split a large path list into chunks no larger than `MAX_PATHS_PER_INVOCATION`.
fn chunk_paths(paths: Vec<PathBuf>) -> Vec<Vec<PathBuf>> {
    if paths.len() <= MAX_PATHS_PER_INVOCATION {
        return vec![paths];
    }
    paths
        .chunks(MAX_PATHS_PER_INVOCATION)
        .map(|c| c.to_vec())
        .collect()
}

#[async_trait]
impl AgentTool for AstEditTool {
    fn name(&self) -> &str {
        "ast_edit"
    }

    fn label(&self) -> &str {
        "AST Edit"
    }

    fn description(&self) -> &str {
        "AST-aware structural code rewriting using ast-grep. Provide an `ops` array of `{pat, out}` pattern→replacement pairs and `paths` to files/dirs/globs to apply them to. Pattern and replacement use ast-grep syntax (e.g. pat='fn $NAME() -> i32 { $BODY }', out='fn $NAME() -> i64 { $BODY }'). Set `dry_run=true` (default) to preview matches without writing; set `dry_run=false` to apply in place. Requires the `sg` CLI on PATH. Globs are expanded by this tool before invoking ast-grep because ast-grep does not expand globs in positional path arguments."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ops": {
                    "type": "array",
                    "description": "Rewrite operations. Each entry maps an ast-grep pattern (`pat`) to a replacement template (`out`). Metavariables in `pat` (e.g. `$NAME`, `$BODY`) are interpolated into `out` by ast-grep.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "pat": {
                                "type": "string",
                                "description": "AST pattern in ast-grep syntax (e.g. 'fn $NAME() -> i32 { $BODY }')."
                            },
                            "out": {
                                "type": "string",
                                "description": "Replacement template (e.g. 'fn $NAME() -> i64 { $BODY }')."
                            }
                        },
                        "required": ["pat", "out"],
                        "additionalProperties": false
                    },
                    "minItems": 1
                },
                "paths": {
                    "type": "array",
                    "description": "Files, directories, or globs to rewrite. Globs (containing `*`, `?`, or `[`) are expanded in-process before invoking ast-grep because ast-grep does not expand globs in positional path arguments.",
                    "items": { "type": "string" },
                    "minItems": 1
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "When true (default), only preview matches — no files are modified. When false, apply the rewrites in place using ast-grep's `--update-all`.",
                    "default": true
                }
            },
            "required": ["ops", "paths"]
        })
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        // Bulk file rewrites that go through `sg -U` mutate many files at
        // once. Defaulting to ParallelSafe would let a concurrent `edit` or
        // another `ast_edit` race on the same paths and corrupt output.
        // Force sequential execution per batch, matching eval_tool /
        // debug_tool / browse_tool. This is correct regardless of the
        // call's dry_run flag (execution_mode is static per-tool, not
        // per-call).
        ToolExecutionMode::SequentialOnly
    }

    fn intent(&self) -> Option<&str> {
        Some("Applying AST rewrites")
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // ── 1. Validate inputs ───────────────────────────────────────
        let ops = parse_ops(&params)?;
        let raw_paths = parse_paths(&params)?;

        // Default dry_run = true (per spec); do NOT copy edit.rs's default.
        let dry_run = params
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        // ── 2. Resolve & expand paths under the tool's root guard ─────
        let root = self.root_dir.as_deref().unwrap_or_else(|| ctx.root());
        let guard = PathGuard::new(root);

        let expanded = Self::expand_paths(&raw_paths, root)?;
        for p in &expanded {
            guard
                .validate(p)
                .map_err(|e| format!("Path '{}' rejected: {}", p.display(), e))?;
        }

        // ── 3. Run each op × each path-chunk ─────────────────────────
        let mut total_replacements: usize = 0;
        let mut total_files_touched: std::collections::BTreeSet<PathBuf> =
            std::collections::BTreeSet::new();
        let mut per_op_summary: Vec<String> = Vec::with_capacity(ops.len());
        let mut had_error = false;
        let mut error_messages: Vec<String> = Vec::new();

        let expanded_count = expanded.len();

        // Chunk the path list once and re-use across every op — avoids
        // re-cloning the (potentially large) expanded list per op.
        let chunks = chunk_paths(expanded);

        for (op_idx, op) in ops.iter().enumerate() {
            let mut op_count: usize = 0;
            let mut op_files: std::collections::BTreeSet<PathBuf> =
                std::collections::BTreeSet::new();

            for chunk in &chunks {
                match run_sg_for_op(op, chunk, dry_run).await {
                    Ok((status, stdout, stderr)) => {
                        if !status.success() {
                            let stderr_text = String::from_utf8_lossy(&stderr).trim().to_string();
                            // Strip the noisy deprecation banner if it's the
                            // only thing on stderr.
                            let cleaned: String = stderr_text
                                .lines()
                                .filter(|l| {
                                    !l.contains("`sg` is deprecated")
                                        && !l.contains("Use `ast-grep` instead")
                                        && !l.starts_with("======")
                                        && !l.trim().is_empty()
                                })
                                .collect::<Vec<_>>()
                                .join(" ");

                            let stdout_text = String::from_utf8_lossy(&stdout).trim().to_string();

                            if !cleaned.is_empty() {
                                had_error = true;
                                error_messages.push(format!(
                                    "ops[{}] (pat='{}') failed (exit {:?}): {}",
                                    op_idx,
                                    op.pat,
                                    status.code(),
                                    cleaned
                                ));
                            } else if !stdout_text.is_empty() {
                                had_error = true;
                                error_messages.push(format!(
                                    "ops[{}] (pat='{}') failed (exit {:?}): {}",
                                    op_idx,
                                    op.pat,
                                    status.code(),
                                    stdout_text
                                ));
                            }
                            // Otherwise: non-zero exit with empty output —
                            // some sg builds signal "no matches" this way.
                            continue;
                        }

                        if dry_run {
                            let (count, by_file) = summarise_dry_run(&stdout);
                            op_count += count;
                            for (f, _n) in by_file {
                                op_files.insert(f);
                            }
                        } else if let Some(n) = parse_applied_count(&stderr) {
                            op_count += n;
                            // sg doesn't tell us which files it touched, so
                            // we assume the entire path-chunk was in scope.
                            for p in chunk {
                                op_files.insert(p.clone());
                            }
                        } else {
                            // Apply succeeded but we couldn't parse the
                            // count — surface what we know.
                            for p in chunk {
                                op_files.insert(p.clone());
                            }
                            error_messages.push(format!(
                                "ops[{}] (pat='{}'): apply succeeded but could not parse 'Applied N changes' from stderr",
                                op_idx, op.pat
                            ));
                        }
                    }
                    Err(msg) => {
                        had_error = true;
                        error_messages.push(format!("ops[{}]: {}", op_idx, msg));
                    }
                }
            }

            total_replacements += op_count;
            for f in &op_files {
                total_files_touched.insert(f.clone());
            }

            let files_label = if op_files.is_empty() {
                "0 files".to_string()
            } else {
                format!("{} location(s)", op_files.len())
            };

            let summary_line = if dry_run {
                format!(
                    "ops[{}] pat='{}': {} match(es) across {}",
                    op_idx, op.pat, op_count, files_label
                )
            } else {
                format!(
                    "ops[{}] pat='{}' → out='{}': {} replacement(s) across {}",
                    op_idx, op.pat, op.out, op_count, files_label
                )
            };
            per_op_summary.push(summary_line);
        }

        // ── 4. Format output ─────────────────────────────────────────
        let header = if dry_run {
            "AST edit preview (dry-run) — no files modified"
        } else {
            "AST edit applied"
        };

        let mut body = String::new();
        body.push_str(header);
        body.push('\n');
        body.push('\n');
        for line in &per_op_summary {
            body.push_str(line);
            body.push('\n');
        }
        body.push('\n');

        if dry_run {
            body.push_str(&format!(
                "Total: {} match(es) across {} location(s)\n",
                total_replacements,
                total_files_touched.len()
            ));
        } else {
            body.push_str(&format!(
                "Total: {} replacement(s) across {} location(s)\n",
                total_replacements,
                total_files_touched.len()
            ));
        }

        if had_error {
            body.push_str("\nErrors:\n");
            for e in &error_messages {
                body.push_str(&format!("  - {}\n", e));
            }
        }

        let trimmed_body = body.trim_end().to_string();

        let mut result = if had_error && total_replacements == 0 {
            AgentToolResult::error(trimmed_body)
        } else {
            AgentToolResult::success(trimmed_body)
        };

        result.metadata = Some(json!({
            "dry_run": dry_run,
            "ops_count": ops.len(),
            "paths_count": expanded_count,
            "total_replacements": total_replacements,
            "locations_touched": total_files_touched.len(),
            "locations": total_files_touched.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "errors": error_messages,
        }));

        Ok(result)
    }
}
