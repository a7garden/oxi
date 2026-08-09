//! TTSR (Time-Traveling Stream Rules) engine.
//!
//! Monitors streaming model output against project rules. When a rule is
//! violated, the stream is aborted and the rule is injected as a system
//! reminder so the model can correct itself.
//!
//! Ported from omp `packages/coding-agent/src/export/ttsr.ts` (TtsrManager).
//!
//! Two matching modes are supported:
//! - **Regex**: the original `condition` field, matched against streaming deltas.
//! - **AST**: the optional `ast_condition` field, matched against file content
//!   after tool writes via the `TtsrAstMatcher`. AST matching shells out to
//!   the `sg` (ast-grep) CLI in the default build — same dependency surface
//!   as [`crate::tools::ast_grep`].

use parking_lot::RwLock;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

// ── Local type definitions (mirrors oxicode_sdk::ports to avoid a dependency cycle) ─

/// Interrupt mode controlling which stream sources TTSR inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterruptMode {
    /// Never interrupt — the rule is informational only.
    Never,
    /// Interrupt only on assistant prose output.
    #[default]
    ProseOnly,
    /// Interrupt only on tool-call arguments.
    ToolOnly,
    /// Interrupt on any source (text, thinking, tools).
    Always,
}

/// Which source produced a TTSR match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeToken {
    /// Assistant prose (text deltas).
    Text,
    /// Model reasoning (thinking blocks).
    Thinking,
    /// Tool argument payload.
    Tool {
        /// Name of the tool whose arguments are being built.
        name: String,
        /// Glob patterns matching affected file paths.
        globs: Vec<String>,
    },
}

/// Where a rule originated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSource {
    /// Shipped with oxicode itself (e.g., rs-future-prelude).
    BuiltinDefaults,
    /// Project-local rule (`.oxicode/rules/*.mdc`).
    Project,
    /// User-level rule (`~/.oxicode/rules/*.mdc`).
    User,
}
/// A single TTSR rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// Human-readable name identifying this rule.
    pub name: String,
    /// The rule body — instructions injected as a system reminder on match.
    pub content: String,
    /// Optional short summary of what the rule governs.
    pub description: Option<String>,
    /// Regex patterns that, when found in the stream, trigger the rule.
    pub condition: Vec<regex::Regex>,
    /// Stream sources (and tools) this rule applies to.
    pub scope: Vec<ScopeToken>,
    /// When the rule is permitted to interrupt the stream.
    pub interrupt_mode: InterruptMode,
    /// Glob patterns restricting the rule to specific file paths.
    pub globs: Vec<String>,
    /// If `true`, the rule is always active regardless of conditions.
    pub always_apply: bool,
    /// Where the rule originated (builtin, project, or user).
    pub source: RuleSource,
    /// Optional ast-grep Smart pattern matched against file content after
    /// tool writes. See `TtsrAstMatcher` for the runtime that consumes
    /// this field. `None` (the common case) means the rule is purely
    /// regex-driven.
    pub ast_condition: Option<String>,
}

/// Registry of TTSR rules (supplied by the host).
///
/// This is a simplified version of `oxicode_sdk::ports::RuleRegistry` that lives
/// in oxicode-agent to avoid a dependency cycle.
pub trait RuleRegistry: Send + Sync + 'static {
    /// Returns a future that resolves to the current set of registered rules.
    fn rules<'a>(&'a self) -> Pin<Box<dyn Future<Output = Vec<Rule>> + Send + 'a>>;

    /// Mark that a rule was injected at a given turn.
    fn mark_injected(&self, _name: &str, _turn: u64) {}

    /// Return all injection records for compaction survival.
    fn injected_records(&self) -> Vec<(String, u64)> {
        vec![]
    }

    /// Restore injection records after compaction.
    fn restore(&self, _records: Vec<(String, u64)>) {}
}
/// Which stream source produced a delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchSource {
    /// Assistant prose (the main response text).
    Text,
    /// Model reasoning (CoT / thinking blocks).
    Thinking,
    /// Tool argument payloads.
    Tool,
}

/// Per-source buffer key.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct BufferKey {
    source: MatchSource,
    /// Only meaningful for Tool source; otherwise None.
    tool_name: Option<String>,
}

// ── Match context ───────────────────────────────────────────────────────────

/// Context passed to [`TtsrEngine::check_delta`] describing what is being
/// generated right now.
#[derive(Debug, Clone)]
pub struct TtsrMatchContext {
    /// Source stream (text / thinking / tool).
    pub source: MatchSource,
    /// Active file paths this delta may affect (for glob-scoped rules).
    pub file_paths: Vec<String>,
    /// Tool name when `source` is [`MatchSource::Tool`].
    pub tool_name: Option<String>,
    /// File contents that have just been written by a tool. Each entry is
    /// `(path, full content)`. Populated by tool execution hooks so the AST
    /// matcher can evaluate `ast_condition` against the new file body.
    /// Empty when no file content is available (the common case during
    /// pure text/thinking streaming).
    pub file_contents: Vec<(String, String)>,
}

// ── AST matcher ─────────────────────────────────────────────────────────────

/// One AST rule: the subset of a [`Rule`] that the `TtsrAstMatcher`
/// cares about. Decoupled from `Rule` so AST evaluation is independent
/// of regex compilation and so the matcher can be reused without a
/// full rule registry.
#[derive(Debug, Clone)]
pub struct AstRule {
    /// Name surfaced on match (mirrors `Rule::name`).
    pub name: String,
    /// ast-grep Smart pattern.
    pub pattern: String,
    /// Glob patterns restricting the rule to specific file paths.
    /// Empty = matches every file path.
    pub file_scope: Vec<String>,
    /// Interrupt mode carried through to the caller.
    pub interrupt_mode: InterruptMode,
}

/// Function signature for the underlying pattern matcher. Given an
/// ast-grep pattern and the file content, return `true` if the pattern
/// matches at least one node in the content.
///
/// The default implementation shells out to the `sg` (ast-grep) CLI —
/// same approach as [`crate::tools::ast_grep`]. Tests inject a
/// pure-Rust matcher so they don't require `sg` to be installed.
pub type AstMatcherFn = dyn Fn(&str, &str) -> bool + Send + Sync;

/// Default matcher: invoke the `sg` CLI. Returns `false` (no match) on
/// any error, including `sg` not being installed. This is deliberately
/// fail-safe — an unreachable binary should not silently break the
/// stream or surface false positives.
fn default_sg_matcher() -> Box<AstMatcherFn> {
    Box::new(|pattern: &str, content: &str| {
        // sg reads source from a path or stdin; we write to a temp
        // file because stdio handling from a sync context is brittle
        // and the `sg` CLI is happiest with a real path. We avoid
        // pulling in the `tempfile` crate (which is only a
        // dev-dependency of oxicode-agent) by hand-rolling a unique
        // path in the system temp directory.
        let mut tmp = std::env::temp_dir();
        let unique = format!(
            "ttsr-ast-{}-{}.snap",
            std::process::id(),
            content_digest(content)
        );
        tmp.push(unique);
        if std::fs::write(&tmp, content).is_err() {
            return false;
        }
        let matched = run_sg_match(pattern, &tmp).unwrap_or(false);
        // Best-effort cleanup; failure to delete the temp file is
        // harmless (the OS reclaims /tmp on reboot).
        let _ = std::fs::remove_file(&tmp);
        matched
    })
}

/// Synchronous wrapper around `sg run -p <pattern> --json <path>`.
///
/// Returns `Ok(true)` if at least one match is produced, `Ok(false)` on
/// no matches or any failure that's not a hard parse error. This mirrors
/// `crate::tools::ast_grep::run_sg` but is sync (we run it from the
/// TTSR call site, not from a tokio task).
fn run_sg_match(pattern: &str, target: &Path) -> std::io::Result<bool> {
    let output = std::process::Command::new("sg")
        .arg("run")
        .arg("-p")
        .arg(pattern)
        .arg("--json")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Ok(false),
    };

    // sg exits 1 on no matches with empty stdout. We treat any exit code
    // with non-empty stdout as "matches present" and any exit code with
    // empty stdout as "no matches".
    Ok(!output.stdout.is_empty())
}

/// AST condition matcher: holds a list of [`AstRule`]s, dedups by file
/// digest, and delegates actual pattern matching to an injectable
/// [`AstMatcherFn`].
pub struct TtsrAstMatcher {
    rules: Vec<AstRule>,
    /// Per-file content digest for dedup: skip when `content` for
    /// `file_path` hasn't changed since the last successful check.
    /// Key is the file path as supplied by the caller; value is a
    /// `DefaultHasher` digest of the file content at the time of the
    /// last successful check.
    seen_digests: HashMap<String, u64>,
    matcher: Box<AstMatcherFn>,
}

impl std::fmt::Debug for TtsrAstMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtsrAstMatcher")
            .field("rules_count", &self.rules.len())
            .field("seen_digests_count", &self.seen_digests.len())
            .finish_non_exhaustive()
    }
}

impl TtsrAstMatcher {
    /// Build a matcher with the default `sg`-backed pattern matcher.
    pub fn new(rules: Vec<AstRule>) -> Self {
        Self {
            rules,
            seen_digests: HashMap::new(),
            matcher: default_sg_matcher(),
        }
    }

    /// Build a matcher with a custom pattern function. Used by tests to
    /// avoid requiring `sg` on PATH.
    pub fn with_matcher(rules: Vec<AstRule>, matcher: Box<AstMatcherFn>) -> Self {
        Self {
            rules,
            seen_digests: HashMap::new(),
            matcher,
        }
    }

    /// Number of registered rules (handy for diagnostics).
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Drop every cached digest. Call when a session restarts so old
    /// file snapshots don't suppress fresh matches.
    pub fn clear_dedup(&mut self) {
        self.seen_digests.clear();
    }

    /// Check one file's content against every AST rule and return the
    /// name of the first rule whose pattern matches.
    ///
    /// Steps:
    /// 1. Filter rules by `file_scope` glob (empty scope = match-all).
    /// 2. Hash the content; skip if this file's digest hasn't changed.
    /// 3. For each surviving rule, run the injected matcher.
    /// 4. Return the first matching rule's name, or `None`.
    ///
    /// The function is infallible by design: any matcher error collapses
    /// to "no match" rather than aborting the stream.
    pub fn check_tool_snapshot(&mut self, file_path: &str, content: &str) -> Option<String> {
        if self.rules.is_empty() {
            return None;
        }

        // ── 1. Glob filter ────────────────────────────────────────
        let candidates: Vec<&AstRule> = self
            .rules
            .iter()
            .filter(|r| file_scope_matches(&r.file_scope, file_path))
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // ── 2. Digest dedup ────────────────────────────────────────
        let digest = content_digest(content);
        if self.seen_digests.get(file_path) == Some(&digest) {
            return None;
        }

        // ── 3. Pattern match (first-hit wins) ──────────────────────
        for rule in candidates {
            if (self.matcher)(&rule.pattern, content) {
                // Record digest only after a successful match so a
                // non-matching snapshot doesn't poison subsequent
                // edits to the same file.
                self.seen_digests.insert(file_path.to_string(), digest);
                return Some(rule.name.clone());
            }
        }

        // No match this round; remember the digest so identical
        // snapshots in later turns are skipped.
        self.seen_digests.insert(file_path.to_string(), digest);
        None
    }
}

/// `DefaultHasher` digest of a string. Stable across runs of the same
/// binary (uses the standard random seed) — dedup is process-local, so
/// cross-run stability is unnecessary.
fn content_digest(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Returns `true` if `file_path` matches any glob in `scope`. Empty
/// scope means "match everything". Each glob is compiled once per call;
/// invalid patterns are silently skipped (consistent with the existing
/// `Rule::globs` handling in `scope_matches`).
fn file_scope_matches(scope: &[String], file_path: &str) -> bool {
    if scope.is_empty() {
        return true;
    }
    scope.iter().any(|g| {
        glob::Pattern::new(g)
            .map(|p| p.matches(file_path))
            .unwrap_or(false)
    })
}

// ── Engine ──────────────────────────────────────────────────────────────────

/// TTSR engine that buffers streaming deltas and checks them against
/// registered rules.
pub struct TtsrEngine {
    rules: Arc<dyn RuleRegistry>,
    /// Per-source accumulation buffers. Keys are source + optional tool name.
    buffers: RwLock<HashMap<BufferKey, Vec<String>>>,
    settings: TtsrSettings,
    /// Optional AST matcher — when `Some`, tool deltas also evaluate
    /// `ast_condition` against the supplied file contents.
    ast_matcher: RwLock<Option<TtsrAstMatcher>>,
}

impl std::fmt::Debug for TtsrEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtsrEngine")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

/// Knobs for the TTSR engine.
#[derive(Debug, Clone)]
pub struct TtsrSettings {
    /// Master on/off switch. When `false`, all checks are no-ops.
    pub enabled: bool,
    /// Default interrupt mode (overridden per-rule).
    pub interrupt_mode: InterruptMode,
    /// Whether the bundled builtin rules are activated.
    pub builtin_rules: bool,
    /// Safety cap: how many times a single turn can be interrupted.
    pub max_retries_per_turn: u32,
}

impl Default for TtsrSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interrupt_mode: InterruptMode::ProseOnly,
            builtin_rules: true,
            max_retries_per_turn: 3,
        }
    }
}

impl TtsrEngine {
    /// Create an engine backed by `rules`.
    pub fn new(rules: Arc<dyn RuleRegistry>, settings: TtsrSettings) -> Self {
        Self {
            rules,
            buffers: RwLock::new(HashMap::new()),
            settings,
            ast_matcher: RwLock::new(None),
        }
    }

    /// Create an engine with an AST matcher pre-installed. The matcher
    /// is queried on tool deltas with `file_contents` present.
    pub fn with_ast_matcher(
        rules: Arc<dyn RuleRegistry>,
        settings: TtsrSettings,
        ast_matcher: TtsrAstMatcher,
    ) -> Self {
        Self {
            rules,
            buffers: RwLock::new(HashMap::new()),
            settings,
            ast_matcher: RwLock::new(Some(ast_matcher)),
        }
    }

    /// Install (or replace) the AST matcher after construction.
    pub fn set_ast_matcher(&self, matcher: TtsrAstMatcher) {
        *self.ast_matcher.write() = Some(matcher);
    }

    /// Drop any installed AST matcher. Subsequent tool deltas skip
    /// AST evaluation.
    pub fn clear_ast_matcher(&self) {
        *self.ast_matcher.write() = None;
    }

    /// Clear all source buffers. Call at the start of each turn.
    pub fn reset_buffers(&self) {
        self.buffers.write().clear();
    }

    /// Append a streaming delta to the appropriate buffer and return any
    /// rules whose conditions now match the accumulated text.
    ///
    /// This is called on every `ProviderEvent::Delta` while streaming.
    pub fn check_delta(&self, delta: &str, ctx: &TtsrMatchContext) -> Vec<Rule> {
        if !self.settings.enabled {
            return vec![];
        }

        let key = self.buffer_key(ctx);
        let mut buffers = self.buffers.write();
        let buf = buffers.entry(key).or_default();
        buf.push(delta.to_string());

        // Join accumulated deltas into one string for matching.
        let full: String = buf.concat();
        let mut matched = self.match_buffer(&full, ctx);

        // ── AST path (tool only) ─────────────────────────────────────
        // AST conditions need file *content*, not streaming deltas. When
        // the caller supplies file_contents, evaluate every (path, content)
        // pair through the installed AST matcher and promote any matching
        // rule to the returned set.
        if matches!(ctx.source, MatchSource::Tool) && !ctx.file_contents.is_empty() {
            let ast_matches = self.check_ast_against_contents(ctx);
            for ast_match in ast_matches {
                if !matched.iter().any(|r| r.name == ast_match.name) {
                    matched.push(ast_match);
                }
            }
        }

        matched
    }

    /// Replace the buffer for `ctx` with a normalized snapshot (used when
    /// tool output is available in a pre-parsed form).
    pub fn check_snapshot(&self, snapshot: &str, ctx: &TtsrMatchContext) -> Vec<Rule> {
        if !self.settings.enabled {
            return vec![];
        }

        let key = self.buffer_key(ctx);
        let mut buffers = self.buffers.write();
        buffers.insert(key, vec![snapshot.to_string()]);

        let mut matched = self.match_buffer(snapshot, ctx);

        // ── AST path ────────────────────────────────────────────────
        if !ctx.file_contents.is_empty() {
            let ast_matches = self.check_ast_against_contents(ctx);
            for ast_match in ast_matches {
                if !matched.iter().any(|r| r.name == ast_match.name) {
                    matched.push(ast_match);
                }
            }
        }

        matched
    }

    /// Return all injected rule records for compaction survival.
    pub fn injected_records(&self) -> Vec<(String, u64)> {
        self.rules.injected_records()
    }

    // ── Private ─────────────────────────────────────────────────────────

    fn buffer_key(&self, ctx: &TtsrMatchContext) -> BufferKey {
        BufferKey {
            source: ctx.source,
            tool_name: if matches!(ctx.source, MatchSource::Tool) {
                ctx.tool_name.clone()
            } else {
                None
            },
        }
    }

    /// Evaluate AST conditions against every (file_path, content) pair
    /// supplied by the caller. Resolves the rule body from the registry
    /// so we have the rule's `content` and `interrupt_mode` for the
    /// returned [`Rule`].
    fn check_ast_against_contents(&self, ctx: &TtsrMatchContext) -> Vec<Rule> {
        let mut guard = self.ast_matcher.write();
        let matcher = match guard.as_mut() {
            Some(m) => m,
            None => return Vec::new(),
        };

        let mut matched = Vec::new();
        for (path, content) in &ctx.file_contents {
            if let Some(rule_name) = matcher.check_tool_snapshot(path, content)
                && let Some(rule) = self.lookup_rule(&rule_name)
            {
                matched.push(rule);
            }
        }
        matched
    }

    /// Look up a rule by name in the registry.
    fn lookup_rule(&self, name: &str) -> Option<Rule> {
        let rules: Vec<Rule> = futures::executor::block_on(self.rules.rules());
        rules.into_iter().find(|r| r.name == name)
    }

    /// Walk every rule and return matching ones (the first match per rule
    /// is sufficient to trigger an interrupt).
    fn match_buffer(&self, buf: &str, ctx: &TtsrMatchContext) -> Vec<Rule> {
        // Rules are cheap to clone (owned strings), but we collect all matches
        // eagerly so the caller can inspect them without holding the lock.
        let mut matched = Vec::new();

        // Collect rules from the registry. We re-fetch each time because
        // rules can be hot-reloaded at runtime.
        let rules: Vec<Rule> = futures::executor::block_on(self.rules.rules());

        for rule in rules {
            // ── Scope filter ──
            if !self.scope_matches(&rule, ctx) {
                continue;
            }

            // ── Interrupt mode filter ──
            let mode = if matches!(rule.interrupt_mode, InterruptMode::Never) {
                self.settings.interrupt_mode
            } else {
                rule.interrupt_mode
            };
            if !self.mode_allows(mode, ctx.source) {
                continue;
            }

            // ── Condition matching ──
            if !rule.condition.iter().any(|re| re.is_match(buf)) {
                continue;
            }

            matched.push(rule);
        }

        matched
    }

    /// Check whether the rule's scope tokens include the current context.
    fn scope_matches(&self, rule: &Rule, ctx: &TtsrMatchContext) -> bool {
        if rule.scope.is_empty() {
            // No scope = applies everywhere.
            return true;
        }

        for token in &rule.scope {
            match token {
                ScopeToken::Text => {
                    if matches!(ctx.source, MatchSource::Text) {
                        return true;
                    }
                }
                ScopeToken::Thinking => {
                    if matches!(ctx.source, MatchSource::Thinking) {
                        return true;
                    }
                }
                ScopeToken::Tool { name, globs } => {
                    if !matches!(ctx.source, MatchSource::Tool) {
                        continue;
                    }
                    if matches!(ctx.tool_name.as_ref(), Some(tool_name) if tool_name != name) {
                        continue;
                    }
                    // If globs are specified, at least one must match a file path.
                    if !globs.is_empty() {
                        let any_match = ctx.file_paths.iter().any(|fp| {
                            globs.iter().any(|g| {
                                // Simple glob: suffix match.
                                g.strip_suffix("/*")
                                    .map(|prefix| fp.starts_with(prefix))
                                    .unwrap_or_else(|| g == fp)
                            })
                        });
                        if !any_match {
                            continue;
                        }
                    }
                    return true;
                }
            }
        }

        false
    }

    /// Check whether the interrupt mode permits firing on this source.
    fn mode_allows(&self, mode: InterruptMode, source: MatchSource) -> bool {
        match mode {
            InterruptMode::Never => false,
            InterruptMode::ProseOnly => matches!(source, MatchSource::Text),
            InterruptMode::ToolOnly => matches!(source, MatchSource::Tool),
            InterruptMode::Always => true,
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::pin::Pin;

    /// Minimal RuleRegistry that returns a static set of rules.
    struct StaticRegistry {
        rules: Vec<Rule>,
        injections: RwLock<Vec<(String, u64)>>,
    }

    impl RuleRegistry for StaticRegistry {
        fn rules<'a>(&'a self) -> Pin<Box<dyn Future<Output = Vec<Rule>> + Send + 'a>> {
            Box::pin(std::future::ready(self.rules.clone()))
        }

        fn mark_injected(&self, name: &str, turn: u64) {
            self.injections.write().push((name.to_string(), turn));
        }

        fn injected_records(&self) -> Vec<(String, u64)> {
            self.injections.read().clone()
        }

        fn restore(&self, records: Vec<(String, u64)>) {
            *self.injections.write() = records;
        }
    }

    fn make_rule(name: &str, pattern: &str) -> Rule {
        Rule {
            name: name.to_string(),
            content: format!("Do not use {pattern}."),
            description: Some(format!("Forbids {pattern}")),
            condition: vec![Regex::new(pattern).unwrap()],
            scope: vec![],
            interrupt_mode: InterruptMode::ProseOnly,
            globs: vec![],
            always_apply: false,
            source: RuleSource::BuiltinDefaults,
            ast_condition: None,
        }
    }

    /// Simple matcher for tests: substring match against the pattern
    /// (with metacharacter-safe semantics). The pattern is treated as a
    /// literal — tests only need to verify orchestration, not actual
    /// ast-grep behavior.
    fn substring_matcher(pattern: &str, content: &str) -> bool {
        content.contains(pattern)
    }

    fn make_ast_rule(name: &str, pattern: &str, scope: Vec<String>) -> AstRule {
        AstRule {
            name: name.to_string(),
            pattern: pattern.to_string(),
            file_scope: scope,
            interrupt_mode: InterruptMode::Always,
        }
    }

    #[test]
    fn test_check_delta_matches_simple_pattern() {
        let rules = Arc::new(StaticRegistry {
            rules: vec![make_rule("no-todo", r"TODO:")],
            injections: RwLock::new(Vec::new()),
        });

        let engine = TtsrEngine::new(
            rules,
            TtsrSettings {
                enabled: true,
                ..Default::default()
            },
        );

        let ctx = TtsrMatchContext {
            source: MatchSource::Text,
            file_paths: vec![],
            tool_name: None,
            file_contents: vec![],
        };

        // First delta — no match yet.
        let results = engine.check_delta("This code is almost ", &ctx);
        assert!(results.is_empty());

        // Second delta triggers the rule.
        let results = engine.check_delta("TODO: fix later", &ctx);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "no-todo");
    }

    #[test]
    fn test_check_delta_respects_disabled() {
        let rules = Arc::new(StaticRegistry {
            rules: vec![make_rule("no-todo", r"TODO:")],
            injections: RwLock::new(Vec::new()),
        });

        let engine = TtsrEngine::new(
            rules,
            TtsrSettings {
                enabled: false, // DISABLED
                ..Default::default()
            },
        );

        let ctx = TtsrMatchContext {
            source: MatchSource::Text,
            file_paths: vec![],
            tool_name: None,
            file_contents: vec![],
        };

        let results = engine.check_delta("TODO: fix later", &ctx);
        assert!(results.is_empty(), "disabled engine must return no matches");
    }

    #[test]
    fn test_scope_filter_respects_tool_scope() {
        let rules = Arc::new(StaticRegistry {
            rules: vec![Rule {
                name: "edit-only-rule".to_string(),
                content: "Only for edit tool".to_string(),
                description: None,
                condition: vec![Regex::new("bad").unwrap()],
                scope: vec![ScopeToken::Tool {
                    name: "edit".to_string(),
                    globs: vec![],
                }],
                interrupt_mode: InterruptMode::Always,
                globs: vec![],
                always_apply: false,
                source: RuleSource::BuiltinDefaults,
                ast_condition: None,
            }],
            injections: RwLock::new(Vec::new()),
        });

        let engine = TtsrEngine::new(
            rules,
            TtsrSettings {
                enabled: true,
                ..Default::default()
            },
        );

        // Text source — scope doesn't match.
        let text_ctx = TtsrMatchContext {
            source: MatchSource::Text,
            file_paths: vec![],
            tool_name: None,
            file_contents: vec![],
        };
        assert!(engine.check_delta("bad code", &text_ctx).is_empty());

        // Tool source matching "edit" — matches.
        let tool_ctx = TtsrMatchContext {
            source: MatchSource::Tool,
            file_paths: vec![],
            tool_name: Some("edit".to_string()),
            file_contents: vec![],
        };
        assert!(!engine.check_delta("bad code", &tool_ctx).is_empty());

        // Tool source but wrong tool name — no match.
        let write_ctx = TtsrMatchContext {
            source: MatchSource::Tool,
            file_paths: vec![],
            tool_name: Some("write".to_string()),
            file_contents: vec![],
        };
        assert!(engine.check_delta("bad code", &write_ctx).is_empty());
    }

    #[test]
    fn test_reset_buffers_clears_accumulation() {
        let rules = Arc::new(StaticRegistry {
            rules: vec![make_rule("no-todo", r"TODO:")],
            injections: RwLock::new(Vec::new()),
        });

        let engine = TtsrEngine::new(
            rules,
            TtsrSettings {
                enabled: true,
                ..Default::default()
            },
        );

        let ctx = TtsrMatchContext {
            source: MatchSource::Text,
            file_paths: vec![],
            tool_name: None,
            file_contents: vec![],
        };

        // Accumulate "TODO" in buffer.
        engine.check_delta("TODO", &ctx);
        // Reset should clear it.
        engine.reset_buffers();

        // Now ":" alone shouldn't match because "TODO" was cleared.
        let results = engine.check_delta(":", &ctx);
        assert!(results.is_empty(), "buffer was reset — TODO should be gone");
    }

    #[test]
    fn test_prose_only_mode_ignores_tool_source() {
        let rules = Arc::new(StaticRegistry {
            rules: vec![make_rule("no-bad", r"bad")],
            injections: RwLock::new(Vec::new()),
        });

        let engine = TtsrEngine::new(
            rules,
            TtsrSettings {
                enabled: true,
                interrupt_mode: InterruptMode::ProseOnly,
                ..Default::default()
            },
        );

        // Text source — allowed.
        let text_ctx = TtsrMatchContext {
            source: MatchSource::Text,
            file_paths: vec![],
            tool_name: None,
            file_contents: vec![],
        };
        assert!(!engine.check_delta("bad code", &text_ctx).is_empty());

        // Tool source — blocked by ProseOnly mode.
        let tool_ctx = TtsrMatchContext {
            source: MatchSource::Tool,
            file_paths: vec![],
            tool_name: Some("edit".to_string()),
            file_contents: vec![],
        };
        assert!(engine.check_delta("bad code", &tool_ctx).is_empty());
    }

    // ── AST matcher tests ──────────────────────────────────────────────

    #[test]
    fn test_ast_match_detects_pattern() {
        // Register a single AST rule: forbid `Box::leak` in any `.rs` file.
        let ast_rules = vec![make_ast_rule(
            "no-box-leak",
            "Box::leak",
            vec!["*.rs".to_string()],
        )];

        let mut matcher = TtsrAstMatcher::with_matcher(ast_rules, Box::new(substring_matcher));

        let content = "fn main() {\n    let _ = Box::leak(Box::new(0));\n}\n";
        let result = matcher.check_tool_snapshot("src/main.rs", content);
        assert_eq!(result.as_deref(), Some("no-box-leak"));
    }

    #[test]
    fn test_ast_match_no_false_positive() {
        // The pattern is NOT present — matcher must return None.
        let ast_rules = vec![make_ast_rule(
            "no-box-leak",
            "Box::leak",
            vec!["*.rs".to_string()],
        )];

        let mut matcher = TtsrAstMatcher::with_matcher(ast_rules, Box::new(substring_matcher));

        let content = "fn main() {\n    println!(\"clean code\");\n}\n";
        let result = matcher.check_tool_snapshot("src/main.rs", content);
        assert!(result.is_none(), "pattern absent — must not match");

        // Dedup: identical content on a re-check still returns None
        // (digest unchanged) — proves dedup doesn't suppress the first
        // non-match either, and is stable on repeat.
        let result = matcher.check_tool_snapshot("src/main.rs", content);
        assert!(result.is_none());

        // Edited content with no pattern still returns None.
        let edited = "fn main() {\n    println!(\"clean code v2\");\n}\n";
        let result = matcher.check_tool_snapshot("src/main.rs", edited);
        assert!(result.is_none());
    }

    #[test]
    fn test_ast_match_respects_file_scope() {
        // Two rules with disjoint scopes; only the matching-scope one fires.
        let ast_rules = vec![
            make_ast_rule("no-rs-leak", "Box::leak", vec!["*.rs".to_string()]),
            make_ast_rule("no-ts-leak", "Box::leak", vec!["*.ts".to_string()]),
        ];

        let mut matcher = TtsrAstMatcher::with_matcher(ast_rules, Box::new(substring_matcher));

        // `.ts` file containing the pattern — `no-ts-leak` must fire,
        // `no-rs-leak` must not (because the `.rs` scope excludes it).
        let ts_content = "export const x = Box::leak(new Object());\n";
        let result = matcher.check_tool_snapshot("app/index.ts", ts_content);
        assert_eq!(result.as_deref(), Some("no-ts-leak"));

        // A `.md` file — neither rule's scope matches, so nothing fires.
        let md_content = "Documentation note: Box::leak is forbidden.\n";
        let result = matcher.check_tool_snapshot("docs/notes.md", md_content);
        assert!(
            result.is_none(),
            "scope filter must exclude out-of-scope files"
        );

        // Empty-scope rule = match-all. Confirms the "empty scope = all"
        // semantic that `Rule::globs` already provides for regex.
        let mut permissive = TtsrAstMatcher::with_matcher(
            vec![make_ast_rule("global", "forbidden-token", vec![])],
            Box::new(substring_matcher),
        );
        let result = permissive.check_tool_snapshot("any/path.xyz", "has forbidden-token here");
        assert_eq!(result.as_deref(), Some("global"));
    }

    #[test]
    fn test_engine_ast_integration_via_tool_delta() {
        // End-to-end: AST match found via `check_delta` on a Tool source
        // with `file_contents` populated. Uses a registry that returns a
        // rule by name so the engine can resolve the matched AST rule
        // back to a full `Rule` for the caller.
        let registry_rules = vec![Rule {
            name: "no-box-leak".to_string(),
            content: "Do not call Box::leak.".to_string(),
            description: None,
            condition: vec![],
            scope: vec![ScopeToken::Tool {
                name: "write".to_string(),
                globs: vec![],
            }],
            interrupt_mode: InterruptMode::Always,
            globs: vec![],
            always_apply: false,
            source: RuleSource::BuiltinDefaults,
            ast_condition: Some("Box::leak".to_string()),
        }];
        let registry: Arc<dyn RuleRegistry> = Arc::new(StaticRegistry {
            rules: registry_rules,
            injections: RwLock::new(Vec::new()),
        });

        let ast_matcher = TtsrAstMatcher::with_matcher(
            vec![make_ast_rule(
                "no-box-leak",
                "Box::leak",
                vec!["*.rs".to_string()],
            )],
            Box::new(substring_matcher),
        );

        let engine = TtsrEngine::with_ast_matcher(
            registry,
            TtsrSettings {
                enabled: true,
                ..Default::default()
            },
            ast_matcher,
        );

        let ctx = TtsrMatchContext {
            source: MatchSource::Tool,
            file_paths: vec!["src/main.rs".to_string()],
            tool_name: Some("write".to_string()),
            file_contents: vec![(
                "src/main.rs".to_string(),
                "fn main() { let _ = Box::leak(Box::new(1)); }\n".to_string(),
            )],
        };

        let matched = engine.check_delta("editing src/main.rs", &ctx);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "no-box-leak");
    }
}
