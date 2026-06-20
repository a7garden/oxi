//! Rule discovery for the TTSR engine.
//!
//! Scans a project directory for rule files and returns compiled
//! [`Rule`] values ready for the TTSR engine. Rules are discovered
//! from several sources:
//!
//! 1. **Bundled builtin rules** — always loaded (see [`builtin_rules`]).
//! 2. **`.oxi/rules/*.mdc`** — project-level rule files.
//! 3. **`.cursorrules/` / `.clinerules/`** — fallback directories
//!    when `.oxi/rules/` does not exist.
//!
//! Each `.mdc` file uses YAML frontmatter followed by a markdown body:
//!
//! ```text
//! ---
//! description: Rule description
//! condition: "regex"
//! scope: "text"
//! interruptMode: prose-only
//! ---
//! Rule body in markdown.
//! ```

use super::builtin_rules;
use oxi_agent::agent_loop::ttsr::Rule;
use oxi_agent::agent_loop::ttsr::RuleSource;
use std::path::Path;

/// Discover all TTSR rules for a project.
///
/// Always includes the bundled default rules. Additionally scans
/// `.oxi/rules/*.mdc` in `project_dir`, falling back to
/// `.cursorrules/` or `.clinerules/` (as directories or single files)
/// when `.oxi/rules/` does not exist.
pub fn discover_rules(project_dir: &Path) -> Vec<Rule> {
    let mut rules = Vec::new();

    // Always include the builtin default rules.
    rules.extend(builtin_rules::load_all());

    let oxi_rules_dir = project_dir.join(".oxi").join("rules");
    if oxi_rules_dir.is_dir() {
        rules.extend(scan_mdc_dir(&oxi_rules_dir, RuleSource::Project));
        return rules;
    }

    // Fallback directories / files.
    for fallback in &[".cursorrules", ".clinerules"] {
        let fallback_path = project_dir.join(fallback);
        if fallback_path.is_dir() {
            rules.extend(scan_mdc_dir(&fallback_path, RuleSource::Project));
        } else if fallback_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&fallback_path) {
                if let Some(rule) =
                    builtin_rules::parse_rule_file(&content, fallback, RuleSource::Project)
                {
                    rules.push(rule);
                }
            }
        }
    }

    rules
}

/// Scan a directory for `.mdc` files and parse them into rules.
fn scan_mdc_dir(dir: &Path, source: RuleSource) -> Vec<Rule> {
    let mut rules = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return rules,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("mdc") {
            continue;
        }

        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Some(rule) = builtin_rules::parse_rule_file(&content, &name, source.clone()) {
            rules.push(rule);
        }
    }

    rules
}

/// A simple [`RuleRegistry`] that serves a static list of rules.
///
/// Used to feed discovered rules into the TTSR engine without
/// requiring a full persisted registry.
pub struct StaticRuleRegistry {
    rules: std::sync::RwLock<Vec<Rule>>,
    injections: std::sync::RwLock<Vec<(String, u64)>>,
}

impl StaticRuleRegistry {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            rules: std::sync::RwLock::new(rules),
            injections: std::sync::RwLock::new(Vec::new()),
        }
    }
}

impl oxi_agent::agent_loop::ttsr::RuleRegistry for StaticRuleRegistry {
    fn rules<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<Rule>> + Send + 'a>> {
        let rules = self.rules.read().unwrap().clone();
        Box::pin(std::future::ready(rules))
    }

    fn mark_injected(&self, name: &str, turn: u64) {
        self.injections
            .write()
            .unwrap()
            .push((name.to_string(), turn));
    }

    fn injected_records(&self) -> Vec<(String, u64)> {
        self.injections.read().unwrap().clone()
    }

    fn restore(&self, records: Vec<(String, u64)>) {
        *self.injections.write().unwrap() = records;
    }
}
