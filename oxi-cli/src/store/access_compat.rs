//! Permission compatibility — partial port of grok-build
//! `xai-grok-workspace/src/permission/` (Apache-2.0).
//!
//! Two pieces are ported at MVP depth:
//!
//! 1. **Bash command splitter** — a lightweight, regex-based variant of
//!    grok's `bash_command_splitting.rs` (which uses tree-sitter-bash).
//!    Detects `&&`, `||`, `;`, `|` boundaries and splits into individual
//!    commands.
//!
//! 2. **`.claude/settings.json` importer** — translates Claude's
//!    `permissions.allow/deny/ask` arrays into oxi `AccessGate` rule
//!    decisions via `ClaudeRuleSet`.
//!
//! ## Non-goals
//!
//! - grok's `auto_mode.rs` (100 KB learning-based auto-approve) is
//!   intentionally NOT ported — it requires training data we don't have.
//! - grok's `manager.rs`/`resolution.rs` (255 KB) are layered on
//!   workspace hooks that oxi doesn't share.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use oxi_sdk::ports::{AccessDecision, ToolCallRequest};

// ── Bash command splitting ──────────────────────────────────────────

/// A single command extracted from a shell pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitCommand {
    /// Original text of this command (leading/trailing whitespace
    /// stripped).
    pub text: String,
    /// Operator that joins this command to the next. `None` on the
    /// last command.
    pub separator: Option<CommandSeparator>,
}

/// Operators that can join commands in a shell pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSeparator {
    /// `cmd1 && cmd2` — run cmd2 only if cmd1 succeeds.
    And,
    /// `cmd1 || cmd2` — run cmd2 only if cmd1 fails.
    Or,
    /// `cmd1 ; cmd2` — run cmd1 then cmd2 unconditionally.
    Sequence,
    /// `cmd1 | cmd2` — pipe cmd1's stdout into cmd2's stdin.
    Pipe,
}

impl CommandSeparator {
    /// The operator string as written in the shell source.
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandSeparator::And => "&&",
            CommandSeparator::Or => "||",
            CommandSeparator::Sequence => ";",
            CommandSeparator::Pipe => "|",
        }
    }
}

/// Split a shell command line on `&&`, `||`, `;`, `|`. Quotes and
/// backslash escapes are honored. Returns `None` on unbalanced quotes.
///
/// The last command always has `separator = None` — operators join a
/// command to the *next* one, so there is no operator after the final
/// command.
pub fn split_bash(input: &str) -> Option<Vec<SplitCommand>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        if c == '\\' && !in_single {
            if let Some(next) = chars.next() {
                current.push('\\');
                current.push(next);
            } else {
                current.push('\\');
            }
            continue;
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            current.push(c);
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            current.push(c);
            continue;
        }

        if !in_single && !in_double {
            if c == '&' && chars.peek() == Some(&'&') {
                chars.next();
                flush_command(&mut current, &mut out, Some(CommandSeparator::And));
                continue;
            }
            if c == '|' {
                if chars.peek() == Some(&'|') {
                    chars.next();
                    flush_command(&mut current, &mut out, Some(CommandSeparator::Or));
                } else {
                    flush_command(&mut current, &mut out, Some(CommandSeparator::Pipe));
                }
                continue;
            }
            if c == ';' {
                flush_command(&mut current, &mut out, Some(CommandSeparator::Sequence));
                continue;
            }
        }
        current.push(c);
    }

    if in_single || in_double {
        return None;
    }

    // Final flush: no separator (no operator after the last command).
    flush_command(&mut current, &mut out, None);
    Some(out)
}

fn flush_command(
    current: &mut String,
    out: &mut Vec<SplitCommand>,
    separator: Option<CommandSeparator>,
) {
    let text = current.trim().to_string();
    if !text.is_empty() {
        out.push(SplitCommand { text, separator });
    }
    current.clear();
}

// ── Claude settings importer ────────────────────────────────────────

/// Subset of `.claude/settings.json` we read.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettings {
    /// `permissions.allow/deny/ask` arrays (optional).
    pub permissions: Option<ParsedPermissions>,
    /// Canonical `defaultMode` string (e.g. `"acceptEdits"`, `"auto"`).
    pub default_mode: Option<String>,
    /// Optional environment variables applied to every session.
    pub env: Option<std::collections::HashMap<String, String>>,
}

/// `permissions` block from Claude settings.
#[derive(Debug, Default, Deserialize)]
pub struct ParsedPermissions {
    /// Tools/commands that always pass without prompting.
    pub allow: Vec<String>,
    /// Tools/commands that are always denied.
    pub deny: Vec<String>,
    /// Tools/commands that always require explicit approval.
    pub ask: Vec<String>,
}

/// A rule extracted from a Claude permissions entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedRule {
    /// Tool name (e.g. `"Bash"`, `"read"`).
    pub tool_name: String,
    /// Argument patterns matched as substrings against the request.
    pub arg_substrings: Vec<String>,
}

/// Outcome of importing `.claude/settings.json`.
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    /// Rules extracted from `permissions.allow`.
    pub allow: Vec<ImportedRule>,
    /// Rules extracted from `permissions.deny`.
    pub deny: Vec<ImportedRule>,
    /// Rules extracted from `permissions.ask`.
    pub ask: Vec<ImportedRule>,
    /// `defaultMode` field if present.
    pub default_mode: Option<String>,
    /// Environment variables if present.
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Rules that could not be parsed. Hosts may surface them as
    /// warnings.
    pub warnings: Vec<String>,
}

/// Read `.claude/settings.json` from `dir/.claude/settings.json`.
pub fn load_claude_settings(dir: &Path) -> Option<ImportResult> {
    let path = claude_settings_path(dir);
    let raw = std::fs::read_to_string(&path).ok()?;
    parse_claude_settings(&raw, &path)
}

/// Standard location of Claude settings relative to a project root.
fn claude_settings_path(dir: &Path) -> PathBuf {
    dir.join(".claude").join("settings.json")
}

/// Parse the raw text of a `.claude/settings.json` file.
pub fn parse_claude_settings(raw: &str, path: &Path) -> Option<ImportResult> {
    let settings: ClaudeSettings = match serde_json::from_str(raw) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to parse Claude settings; skipping"
            );
            return None;
        }
    };
    Some(translate(settings))
}

/// Translate a parsed [`ClaudeSettings`] to the rule-list form.
fn translate(settings: ClaudeSettings) -> ImportResult {
    let mut out = ImportResult {
        default_mode: settings.default_mode,
        env: settings.env,
        ..Default::default()
    };
    if let Some(perms) = settings.permissions {
        for (action, entries, target) in [
            ("allow", perms.allow, &mut out.allow),
            ("deny", perms.deny, &mut out.deny),
            ("ask", perms.ask, &mut out.ask),
        ] {
            for entry in entries {
                match parse_rule_entry(&entry) {
                    Some(rule) => target.push(rule),
                    None => out
                        .warnings
                        .push(format!("{action}: could not parse rule {entry:?}")),
                }
            }
        }
    }
    out
}

/// Parse a single Claude permissions entry string.
///
/// Format: `ToolName` (allow all) or `ToolName(arg1, arg2)` (allow only
/// when the request fields contain those substrings).
pub fn parse_rule_entry(entry: &str) -> Option<ImportedRule> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    if let Some(open) = entry.find('(') {
        let close = entry.rfind(')')?;
        if close <= open {
            return None;
        }
        let tool_name = entry[..open].trim().to_string();
        let args_str = &entry[open + 1..close];
        let arg_substrings: Vec<String> = args_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Some(ImportedRule {
            tool_name,
            arg_substrings,
        })
    } else {
        Some(ImportedRule {
            tool_name: entry.to_string(),
            arg_substrings: Vec::new(),
        })
    }
}

/// Adapter that wires an [`ImportResult`] into an `AccessGate`-like
/// check.
///
/// Substring matching scans `req.action`, `req.subject`, and `req.cwd`
/// for each needle (any-match).
pub struct ClaudeRuleSet {
    allow: Vec<ImportedRule>,
    deny: Vec<ImportedRule>,
    ask: Vec<ImportedRule>,
}

impl ClaudeRuleSet {
    /// Build from a successful import.
    pub fn from_import(import: ImportResult) -> Self {
        Self {
            allow: import.allow,
            deny: import.deny,
            ask: import.ask,
        }
    }

    /// Evaluate a tool call request. Priority: deny > ask > allow >
    /// default.
    pub fn evaluate(&self, req: &ToolCallRequest) -> AccessDecision {
        if self.matches(&self.deny, req) {
            return AccessDecision::Deny {
                reason: format!("denied by .claude/settings.json rule (tool {})", req.tool),
            };
        }
        if self.matches(&self.ask, req) {
            return AccessDecision::RequireApproval {
                reason: format!(
                    "requires approval per .claude/settings.json rule (tool {})",
                    req.tool
                ),
            };
        }
        if self.matches(&self.allow, req) {
            return AccessDecision::Allow;
        }
        if self.allow.is_empty() {
            AccessDecision::Deny {
                reason: "no allow rule matches and allow-list is empty".into(),
            }
        } else {
            AccessDecision::Allow
        }
    }

    fn matches(&self, rules: &[ImportedRule], req: &ToolCallRequest) -> bool {
        rules.iter().any(|r| {
            r.tool_name == req.tool
                && r.arg_substrings.iter().all(|needle| {
                    let needle = needle.as_str();
                    needle.is_empty()
                        || req.action.contains(needle)
                        || req.subject.contains(needle)
                        || req.cwd.to_string_lossy().contains(needle)
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(tool: &str, action: &str) -> ToolCallRequest {
        ToolCallRequest {
            tool: tool.to_string(),
            action: action.to_string(),
            cwd: PathBuf::from("/tmp"),
            subject: "test".to_string(),
        }
    }

    #[test]
    fn split_single_command() {
        let cmds = split_bash("ls -la").unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].text, "ls -la");
        assert_eq!(cmds[0].separator, None);
    }

    #[test]
    fn split_and_chain() {
        let cmds = split_bash("cargo build && cargo test").unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].text, "cargo build");
        assert_eq!(cmds[0].separator, Some(CommandSeparator::And));
        assert_eq!(cmds[1].text, "cargo test");
        assert_eq!(cmds[1].separator, None);
    }

    #[test]
    fn split_or_sequence_pipe() {
        let cmds = split_bash("a || b ; c | d").unwrap();
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[0].separator, Some(CommandSeparator::Or));
        assert_eq!(cmds[1].separator, Some(CommandSeparator::Sequence));
        assert_eq!(cmds[2].separator, Some(CommandSeparator::Pipe));
        assert_eq!(cmds[3].separator, None);
    }

    #[test]
    fn split_preserves_quoted_operators() {
        let cmds = split_bash(r#"echo "a | b" && ls"#).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].text, r#"echo "a | b""#);
    }

    #[test]
    fn split_preserves_single_quoted_operators() {
        let cmds = split_bash("echo 'a && b' && ls").unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].text, "echo 'a && b'");
    }

    #[test]
    fn split_handles_escape() {
        let cmds = split_bash(r"a \|\| b").unwrap();
        assert_eq!(cmds.len(), 1, "escaped || must not split");
        assert_eq!(cmds[0].text, r"a \|\| b");
    }

    #[test]
    fn split_unbalanced_quote_returns_none() {
        assert!(split_bash("echo 'unbalanced").is_none());
    }

    #[test]
    fn split_empty_input_yields_empty() {
        let cmds = split_bash("").unwrap();
        assert!(cmds.is_empty());
    }

    #[test]
    fn split_whitespace_only_yields_empty() {
        let cmds = split_bash("    \n   ").unwrap();
        assert!(cmds.is_empty());
    }

    #[test]
    fn parse_rule_bare_tool() {
        let r = parse_rule_entry("read").unwrap();
        assert_eq!(r.tool_name, "read");
        assert!(r.arg_substrings.is_empty());
    }

    #[test]
    fn parse_rule_with_args() {
        let r = parse_rule_entry(r#"Bash(npm install:*)"#).unwrap();
        assert_eq!(r.tool_name, "Bash");
        assert_eq!(r.arg_substrings, vec!["npm install:*"]);
    }

    #[test]
    fn parse_rule_multiple_args() {
        let r = parse_rule_entry("Bash(git, push)").unwrap();
        assert_eq!(r.tool_name, "Bash");
        assert_eq!(r.arg_substrings, vec!["git", "push"]);
    }

    #[test]
    fn parse_rule_unbalanced_returns_none() {
        assert!(parse_rule_entry("Bash(git").is_none());
    }

    #[test]
    fn parse_claude_settings_full() {
        let raw = r#"{
            "permissions": {
                "allow": ["read", "Bash(npm test)"],
                "deny": ["Bash(rm -rf)"],
                "ask": ["write(/etc/*)"]
            },
            "defaultMode": "default",
            "env": {"FOO": "bar"}
        }"#;
        let import = parse_claude_settings(raw, Path::new("/test/.claude/settings.json")).unwrap();
        assert_eq!(import.allow.len(), 2);
        assert_eq!(import.allow[0].tool_name, "read");
        assert_eq!(import.allow[1].tool_name, "Bash");
        assert_eq!(import.deny.len(), 1);
        assert_eq!(import.ask.len(), 1);
        assert_eq!(import.default_mode.as_deref(), Some("default"));
        assert_eq!(
            import.env.as_ref().unwrap().get("FOO").map(|s| s.as_str()),
            Some("bar")
        );
    }

    #[test]
    fn parse_claude_settings_invalid_json_returns_none() {
        let raw = "not json at all";
        assert!(parse_claude_settings(raw, Path::new("/test/settings.json")).is_none());
    }

    #[test]
    fn parse_claude_settings_empty() {
        let raw = "{}";
        let import = parse_claude_settings(raw, Path::new("/test/settings.json")).unwrap();
        assert!(import.allow.is_empty());
        assert!(import.deny.is_empty());
        assert_eq!(import.warnings.len(), 0);
    }

    #[test]
    fn evaluate_deny_wins() {
        let import = ImportResult {
            deny: vec![ImportedRule {
                tool_name: "Bash".into(),
                arg_substrings: vec!["rm -rf".into()],
            }],
            ..Default::default()
        };
        let set = ClaudeRuleSet::from_import(import);
        let decision = set.evaluate(&req("Bash", "rm -rf /tmp/x"));
        assert!(matches!(decision, AccessDecision::Deny { .. }));
    }

    #[test]
    fn evaluate_ask_then_allow() {
        let import = ImportResult {
            ask: vec![ImportedRule {
                tool_name: "write".into(),
                arg_substrings: vec!["/etc/".into()],
            }],
            allow: vec![ImportedRule {
                tool_name: "write".into(),
                arg_substrings: vec!["/tmp/".into()],
            }],
            ..Default::default()
        };
        let set = ClaudeRuleSet::from_import(import);
        let ask = set.evaluate(&req("write", "edit /etc/passwd"));
        assert!(matches!(ask, AccessDecision::RequireApproval { .. }));
        let allow = set.evaluate(&req("write", "edit /tmp/x"));
        assert!(matches!(allow, AccessDecision::Allow));
    }

    #[test]
    fn evaluate_empty_allow_list_denies() {
        let set = ClaudeRuleSet::from_import(ImportResult::default());
        let decision = set.evaluate(&req("read", "open README"));
        assert!(matches!(decision, AccessDecision::Deny { .. }));
    }

    #[test]
    fn evaluate_nonempty_allow_default_allows() {
        let import = ImportResult {
            allow: vec![ImportedRule {
                tool_name: "read".into(),
                arg_substrings: vec!["/tmp/".into()],
            }],
            ..Default::default()
        };
        let set = ClaudeRuleSet::from_import(import);
        let decision = set.evaluate(&req("read", "open /etc/x"));
        assert!(matches!(decision, AccessDecision::Allow));
    }
}
