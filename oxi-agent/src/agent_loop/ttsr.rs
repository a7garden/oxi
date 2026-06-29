//! TTSR (Time-Traveling Stream Rules) engine.
//!
//! Monitors streaming model output against project rules. When a rule is
//! violated, the stream is aborted and the rule is injected as a system
//! reminder so the model can correct itself.
//!
//! Ported from omp `packages/coding-agent/src/export/ttsr.ts` (TtsrManager).
//!
//! Only regex-based matching is implemented in the default build.
//! AST-based matching (`astCondition`) requires an additional parser dependency
//! and is deferred to a future feature gate.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ── Local type definitions (mirrors oxi_sdk::ports to avoid a dependency cycle) ─

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
    /// Shipped with oxi itself (e.g., rs-future-prelude).
    BuiltinDefaults,
    /// Project-local rule (`.oxi/rules/*.mdc`).
    Project,
    /// User-level rule (`~/.oxi/rules/*.mdc`).
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
}

/// Registry of TTSR rules (supplied by the host).
///
/// This is a simplified version of `oxi_sdk::ports::RuleRegistry` that lives
/// in oxi-agent to avoid a dependency cycle.
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
}

// ── Engine ──────────────────────────────────────────────────────────────────

/// TTSR engine that buffers streaming deltas and checks them against
/// registered rules.
pub struct TtsrEngine {
    rules: Arc<dyn RuleRegistry>,
    /// Per-source accumulation buffers. Keys are source + optional tool name.
    buffers: RwLock<HashMap<BufferKey, Vec<String>>>,
    settings: TtsrSettings,
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
        }
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
        self.match_buffer(&full, ctx).into_iter().collect()
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

        self.match_buffer(snapshot, ctx).into_iter().collect()
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
        };
        assert!(engine.check_delta("bad code", &text_ctx).is_empty());

        // Tool source matching "edit" — matches.
        let tool_ctx = TtsrMatchContext {
            source: MatchSource::Tool,
            file_paths: vec![],
            tool_name: Some("edit".to_string()),
        };
        assert!(!engine.check_delta("bad code", &tool_ctx).is_empty());

        // Tool source but wrong tool name — no match.
        let write_ctx = TtsrMatchContext {
            source: MatchSource::Tool,
            file_paths: vec![],
            tool_name: Some("write".to_string()),
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
        };
        assert!(!engine.check_delta("bad code", &text_ctx).is_empty());

        // Tool source — blocked by ProseOnly mode.
        let tool_ctx = TtsrMatchContext {
            source: MatchSource::Tool,
            file_paths: vec![],
            tool_name: Some("edit".to_string()),
        };
        assert!(engine.check_delta("bad code", &tool_ctx).is_empty());
    }
}
