//! Compile-time embedded TTSR rules for Rust projects.
//!
//! Each `.md` file in this directory is a TTSR rule with YAML
//! frontmatter and markdown body. They are embedded at compile time
//! via `include_str!()`.

use oxi_agent::agent_loop::ttsr::{InterruptMode, Rule, RuleSource, ScopeToken};
use regex::Regex;
use serde::Deserialize;

/// Raw content of bundled rule files: (filename stem, content).
fn raw_bundled_rules() -> Vec<(&'static str, &'static str)> {
    vec![
        ("rs-future-prelude", include_str!("rs-future-prelude.md")),
        ("rs-box-leak", include_str!("rs-box-leak.md")),
        (
            "rs-match-ergonomics",
            include_str!("rs-match-ergonomics.md"),
        ),
        ("rs-parking-lot", include_str!("rs-parking-lot.md")),
        ("rs-result-type", include_str!("rs-result-type.md")),
        ("rs-lazylock", include_str!("rs-lazylock.md")),
        ("rs-tokio-mutex", include_str!("rs-tokio-mutex.md")),
    ]
}

/// YAML frontmatter parsed from a `.mdc` rule file.
#[derive(Debug, Deserialize)]
struct RuleFrontmatter {
    description: Option<String>,
    condition: String,
    scope: Option<String>,
    #[serde(rename = "interruptMode")]
    interrupt_mode: Option<String>,
    globs: Option<Vec<String>>,
    #[serde(rename = "alwaysApply")]
    always_apply: Option<bool>,
}

/// Map a frontmatter scope string to [`ScopeToken`] values.
fn parse_scope(s: &str) -> Vec<ScopeToken> {
    match s.trim().to_lowercase().as_str() {
        "text" => vec![ScopeToken::Text],
        "thinking" => vec![ScopeToken::Thinking],
        "tool" => vec![ScopeToken::Tool {
            name: String::new(),
            globs: vec![],
        }],
        _ => vec![ScopeToken::Text],
    }
}

/// Map a frontmatter interrupt-mode string to [`InterruptMode`].
fn parse_interrupt_mode(s: &str) -> InterruptMode {
    match s.trim().to_lowercase().as_str() {
        "never" => InterruptMode::Never,
        "prose-only" | "proseonly" => InterruptMode::ProseOnly,
        "tool-only" | "toolonly" => InterruptMode::ToolOnly,
        "always" => InterruptMode::Always,
        _ => InterruptMode::ProseOnly,
    }
}

/// Parse a single rule file from its raw content.
///
/// Returns `None` if the frontmatter is missing, malformed, or the
/// regex condition fails to compile.
pub(crate) fn parse_rule_file(content: &str, name: &str, source: RuleSource) -> Option<Rule> {
    // Split on the first two `---` separators (frontmatter delimiters).
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }

    let yaml_str = parts[1].trim();
    let body = parts[2].trim();

    let fm: RuleFrontmatter = serde_yaml::from_str(yaml_str).ok()?;
    let condition = Regex::new(&fm.condition).ok()?;

    let scope = parse_scope(fm.scope.as_deref().unwrap_or("text"));
    let interrupt_mode = parse_interrupt_mode(fm.interrupt_mode.as_deref().unwrap_or("prose-only"));

    Some(Rule {
        name: name.to_string(),
        content: body.to_string(),
        description: fm.description,
        condition: vec![condition],
        scope,
        interrupt_mode,
        globs: fm.globs.unwrap_or_default(),
        always_apply: fm.always_apply.unwrap_or(false),
        source,
    })
}

/// Load all bundled builtin rules.
///
/// These are the Rust-specific rules shipped with oxi-cli. They are
/// always available regardless of project configuration.
pub fn load_all() -> Vec<Rule> {
    raw_bundled_rules()
        .into_iter()
        .filter_map(|(name, content)| parse_rule_file(content, name, RuleSource::BuiltinDefaults))
        .collect()
}
