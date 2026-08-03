//! Model roles — named model assignments ported from omp's `model-roles.ts`.
//!
//! Pure data + string resolution. There is **no** `model_db` / `Model`
//! dependency here: this module resolves a role to one or more model
//! *pattern strings* (e.g. `"anthropic/claude-haiku"`, `"pi/slow:high"`).
//! Converting those strings into a concrete [`crate::Model`] — and the
//! role-switching layer that decides *which* role is active — is the
//! consumer's job (deferred).
//!
//! Ported from omp `packages/coding-agent/src/config/model-roles.ts` (the
//! `ModelRole` type + `MODEL_ROLES` metadata table) and the role-alias
//! resolution in `packages/coding-agent/src/config/model-resolver.ts`
//! (`resolveConfiguredRolePattern` / `resolveDefaultInheritedPatterns`).
//!
//! What is **not** ported: omp's `MODEL_PRIO` built-in default model chains.
//! Those hardcode omp-catalog model ids that do not exist in oxicode; oxicode leaves
//! built-in defaults empty (see [`RoleRegistry::builtin_defaults`]) for the
//! switching layer to fill via oxicode's own catalog / `FallbackChain`.

//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), which is MIT licensed
//! (Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! see the omp repository `LICENSE`). oxicode's translation remains under oxicode's
//! own MIT license.
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

// ── Live role registry ─────────────────────────────────────────────
/// Process-wide live role registry, shared between the [`crate::role_routing`]
/// provider and the settings UI. Installed once at bootstrap; mutated in place
/// by the UI so edits apply to the next `stream()` call without rewrapping the
/// provider (mirrors the global model-registry pattern).
static LIVE_ROLE_REGISTRY: OnceLock<Arc<RwLock<RoleRegistry>>> = OnceLock::new();

/// Install the live role registry. Called once at bootstrap; subsequent calls
/// are no-ops (the first installation wins).
pub fn set_live_role_registry(registry: Arc<RwLock<RoleRegistry>>) {
    let _ = LIVE_ROLE_REGISTRY.set(registry);
}

/// Access the live role registry, if installed.
#[must_use]
pub fn live_role_registry() -> Option<&'static Arc<RwLock<RoleRegistry>>> {
    LIVE_ROLE_REGISTRY.get()
}

/// Prefix marking a role-alias reference, ported from omp's `pi/`.
///
/// A model-pattern string of the form `pi/<role>` is treated as a reference
/// to another role (e.g. `pi/smol`, `pi/slow:high`) and expanded by
/// [`RoleRegistry::resolve`]. Any other string is a concrete model id.
pub const ROLE_ALIAS_PREFIX: &str = "pi/";

/// The 10 built-in model roles, ported verbatim from omp's `ModelRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    /// Main conversational model.
    Default,
    /// Small / fast / cheap model — background tasks, quick completions.
    Smol,
    /// Heavy reasoning ("thinking") model.
    Slow,
    /// Image-input ("vision") capable model.
    Vision,
    /// Architecture / planning model.
    Plan,
    /// Design-oriented model.
    Designer,
    /// Commit-message generation model.
    Commit,
    /// Session-title generation model (hidden from the selector UI).
    Title,
    /// Subagent / task delegation model.
    Task,
    /// Background advisor model.
    Advisor,
}

impl ModelRole {
    /// All built-in roles in omp declaration order (matches `MODEL_ROLE_IDS`).
    pub const ALL: [ModelRole; 10] = [
        ModelRole::Default,
        ModelRole::Smol,
        ModelRole::Slow,
        ModelRole::Vision,
        ModelRole::Plan,
        ModelRole::Designer,
        ModelRole::Commit,
        ModelRole::Title,
        ModelRole::Task,
        ModelRole::Advisor,
    ];

    /// Lowercase identifier, matching omp's role strings and config keys.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ModelRole::Default => "default",
            ModelRole::Smol => "smol",
            ModelRole::Slow => "slow",
            ModelRole::Vision => "vision",
            ModelRole::Plan => "plan",
            ModelRole::Designer => "designer",
            ModelRole::Commit => "commit",
            ModelRole::Title => "title",
            ModelRole::Task => "task",
            ModelRole::Advisor => "advisor",
        }
    }

    /// Parse a lowercase role id. Returns `None` for unknown / custom names.
    #[must_use]
    pub fn from_id(s: &str) -> Option<Self> {
        Some(match s {
            "default" => ModelRole::Default,
            "smol" => ModelRole::Smol,
            "slow" => ModelRole::Slow,
            "vision" => ModelRole::Vision,
            "plan" => ModelRole::Plan,
            "designer" => ModelRole::Designer,
            "commit" => ModelRole::Commit,
            "title" => ModelRole::Title,
            "task" => ModelRole::Task,
            "advisor" => ModelRole::Advisor,
            _ => return None,
        })
    }
}

/// Role color tag, ported from omp's theme color names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoleColor {
    /// `success` — default role.
    Success,
    /// `warning` — fast / cheap role.
    Warning,
    /// `accent` — reasoning / advisor role.
    Accent,
    /// `error` — vision role.
    Error,
    /// `muted` — planning / subtask roles. (Default for custom roles.)
    #[default]
    Muted,
    /// `dim` — low-prominence roles (commit, title).
    Dim,
}

/// Display metadata for a role, ported from omp's `ModelRoleInfo`.
#[derive(Debug, Clone)]
pub struct RoleInfo {
    /// Short uppercase tag (e.g. `"SMOL"`), or `None` for custom roles.
    pub tag: Option<&'static str>,
    /// Human-readable name (e.g. `"Fast"`).
    pub name: &'static str,
    /// Theme color slot.
    pub color: RoleColor,
    /// If `true`, the role is functional but hidden from the selector UI.
    pub hidden: bool,
}

/// Built-in metadata for a role — omp's `MODEL_ROLES` table.
#[must_use]
pub fn builtin_role_info(role: ModelRole) -> RoleInfo {
    match role {
        ModelRole::Default => RoleInfo {
            tag: Some("DEFAULT"),
            name: "Default",
            color: RoleColor::Success,
            hidden: false,
        },
        ModelRole::Smol => RoleInfo {
            tag: Some("SMOL"),
            name: "Fast",
            color: RoleColor::Warning,
            hidden: false,
        },
        ModelRole::Slow => RoleInfo {
            tag: Some("SLOW"),
            name: "Thinking",
            color: RoleColor::Accent,
            hidden: false,
        },
        ModelRole::Vision => RoleInfo {
            tag: Some("VISION"),
            name: "Vision",
            color: RoleColor::Error,
            hidden: false,
        },
        ModelRole::Plan => RoleInfo {
            tag: Some("PLAN"),
            name: "Architect",
            color: RoleColor::Muted,
            hidden: false,
        },
        ModelRole::Designer => RoleInfo {
            tag: Some("DESIGNER"),
            name: "Designer",
            color: RoleColor::Muted,
            hidden: false,
        },
        ModelRole::Commit => RoleInfo {
            tag: Some("COMMIT"),
            name: "Commit",
            color: RoleColor::Dim,
            hidden: false,
        },
        ModelRole::Title => RoleInfo {
            tag: Some("TITLE"),
            name: "Title",
            color: RoleColor::Dim,
            hidden: true,
        },
        ModelRole::Task => RoleInfo {
            tag: Some("TASK"),
            name: "Subtask",
            color: RoleColor::Muted,
            hidden: false,
        },
        ModelRole::Advisor => RoleInfo {
            tag: Some("ADVISOR"),
            name: "Advisor",
            color: RoleColor::Accent,
            hidden: false,
        },
    }
}

/// Visible built-in role ids (hidden ones excluded), the base of omp's
/// `getKnownRoleIds`. Custom roles configured by the user are appended by
/// [`RoleRegistry::known_ids`].
#[must_use]
pub fn builtin_visible_ids() -> Vec<&'static str> {
    ModelRole::ALL
        .iter()
        .filter(|r| !builtin_role_info(**r).hidden)
        .map(|r| r.as_str())
        .collect()
}

/// Recognized thinking-level suffix keywords. A model pattern may carry a
/// trailing `:<level>` (e.g. `pi/slow:high`) which is preserved through
/// alias expansion and re-attached to the resolved pattern. Matches oxicode's
/// [`crate::ThinkingLevel`] variants.
const THINKING_SUFFIXES: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh"];

/// Parse a `pi/<role>` alias reference, ported from omp's `getModelRoleAlias`.
///
/// Returns the built-in role id when `value` is exactly `pi/<known-role>`.
/// A `pi/<custom>` or non-`pi/` string is **not** an alias (returns `None`)
/// and is treated as a concrete model pattern.
fn parse_role_alias(value: &str) -> Option<&str> {
    let normalized = value.trim();
    let candidate = normalized.strip_prefix(ROLE_ALIAS_PREFIX)?;
    // Only built-in role ids are aliases; custom role names are concrete ids.
    if ModelRole::from_id(candidate).is_some() {
        Some(candidate)
    } else {
        None
    }
}

/// Split a trailing `:<level>` thinking suffix, ported from omp's
/// `splitThinkingSuffix`. Only splits when the suffix after the *last* colon
/// is a recognized thinking level — so provider routing variants like
/// OpenRouter's `:nitro` / `:exacto` stay attached to the id.
///
/// Returns `(base, Some(level))` or `(original, None)`.
fn split_thinking_suffix(pattern: &str) -> (&str, Option<&str>) {
    let Some((base, suffix)) = pattern.rsplit_once(':') else {
        return (pattern, None);
    };
    if THINKING_SUFFIXES.contains(&suffix) {
        (base, Some(suffix))
    } else {
        (pattern, None)
    }
}

/// Split a comma-separated pattern list into trimmed, non-empty entries.
fn normalize_pattern_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Whether an unset role inherits the `default` role's patterns before
/// falling back, ported from omp's `shouldInheritDefaultBeforePriority`.
fn inherits_default(role: &str) -> bool {
    matches!(role, "smol" | "slow" | "designer")
}

/// Role registry: maps role names to configured model patterns, with
/// `pi/<role>` alias expansion and cycle detection.
///
/// String-keyed so custom roles (beyond the 10 built-ins) are accepted, just
/// like omp's `modelRoles: Record<string, string>`.
#[derive(Debug, Clone, Default)]
pub struct RoleRegistry {
    /// Role name → model pattern (e.g. `"anthropic/claude-haiku"` or `"pi/slow:high"`).
    roles: HashMap<String, String>,
}

impl RoleRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry from a `role → pattern` map (e.g. parsed from settings).
    #[must_use]
    pub fn from_map(roles: HashMap<String, String>) -> Self {
        Self { roles }
    }

    /// Get the configured pattern for a role, if any.
    #[must_use]
    pub fn get(&self, role: &str) -> Option<&str> {
        self.roles.get(role).map(String::as_str)
    }

    /// Assign a model pattern to a role.
    pub fn set(&mut self, role: impl Into<String>, model: impl Into<String>) {
        self.roles.insert(role.into(), model.into());
    }

    /// Whether any role is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }

    /// Iterate over `(role, pattern)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.roles.iter()
    }

    /// Known role ids: visible built-ins first (in declaration order), then
    /// any configured custom roles sorted by name — ported from omp's
    /// `getKnownRoleIds`.
    #[must_use]
    pub fn known_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = builtin_visible_ids()
            .into_iter()
            .map(String::from)
            .collect();
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        let mut customs: Vec<&String> = self.roles.keys().filter(|r| !seen.contains(*r)).collect();
        customs.sort();
        for role in customs {
            seen.insert(role.clone());
            out.push(role.clone());
        }
        out.into_iter()
            .filter(|r| !seen_is_hidden(r))
            .collect::<Vec<_>>()
    }

    /// Resolve a role to concrete model pattern(s), expanding `pi/<role>`
    /// aliases with cycle detection. Ported from omp's
    /// `resolveConfiguredRolePattern`.
    ///
    /// - A directly-configured role yields its (alias-expanded) patterns.
    /// - An unset `smol` / `slow` / `designer` inherits the `default` role's
    ///   patterns (which themselves may alias another role).
    /// - Self-aliases (`default = "pi/default"`) collapse to the built-in
    ///   default chain (empty in oxicode — see [`Self::builtin_defaults`]).
    /// - A cycle (`a → pi/b`, `b → pi/a`) terminates, returning what
    ///   resolved before the cycle closed.
    /// - An entirely unset role with no inheritance returns an empty vec.
    #[must_use]
    pub fn resolve(&self, role: &str) -> Vec<String> {
        let mut visited: HashSet<String> = HashSet::new();
        self.resolve_role(role, &mut visited)
    }

    /// Built-in default model chain for a role.
    ///
    /// omp seeds these from `MODEL_PRIO` (omp-catalog ids). oxicode returns an
    /// empty list: the switching layer (deferred) fills defaults via oxicode's
    /// own catalog / `FallbackChain`. This keeps the *machinery* faithful
    /// without embedding ids that don't exist in oxicode.
    #[must_use]
    pub fn builtin_defaults(&self, _role: &str) -> Vec<String> {
        Vec::new()
    }

    /// Resolve a role name to concrete model patterns. Cycle-safe.
    ///
    /// Raw patterns come from the configured value, or (for `smol` / `slow` /
    /// `designer`) the inherited `default` value. Each pattern is then fully
    /// expanded: concrete model ids pass through; `pi/<role>` aliases recurse.
    /// Cyclic or otherwise unresolvable aliases are dropped — a dangling role
    /// reference is not a usable model id.
    ///
    /// Divergence from omp: omp's `resolveConfiguredRolePattern` leaves
    /// `pi/<role>` aliases inside a configured value for a downstream pass to
    /// expand; oxicode expands them inline so [`Self::resolve`] always yields
    /// concrete model patterns.
    fn resolve_role(&self, role: &str, visited: &mut HashSet<String>) -> Vec<String> {
        if visited.contains(role) {
            return Vec::new();
        }
        visited.insert(role.to_string());

        let role_defaults = self.builtin_defaults(role);

        // Raw patterns for this role: the configured value, else (for the
        // inheriting roles) the `default` role's value, else none.
        let raw: Vec<String> = if let Some(cfg) = self.roles.get(role) {
            normalize_pattern_list(cfg)
        } else if inherits_default(role) && self.roles.contains_key(ModelRole::Default.as_str()) {
            normalize_pattern_list(&self.roles[ModelRole::Default.as_str()])
        } else {
            Vec::new()
        };

        let mut resolved = Vec::new();
        for pattern in raw {
            resolved.extend(self.expand_pattern(&pattern, visited));
        }
        if resolved.is_empty() {
            resolved = role_defaults;
        }
        resolved
    }

    /// Expand a single pattern value: a concrete model id yields itself; a
    /// `pi/<role>[:<level>]` alias recurses into [`Self::resolve_role`] with
    /// the thinking suffix re-attached to every resolved pattern.
    fn expand_pattern(&self, pattern: &str, visited: &mut HashSet<String>) -> Vec<String> {
        let normalized = pattern.trim();
        if normalized.is_empty() {
            return Vec::new();
        }
        let (base, thinking_level) = split_thinking_suffix(normalized);
        match parse_role_alias(base) {
            None => vec![normalized.to_string()],
            Some(alias) => {
                let mut expanded = self.resolve_role(alias, visited);
                if let Some(level) = thinking_level {
                    expanded = expanded
                        .into_iter()
                        .map(|p| format!("{p}:{level}"))
                        .collect();
                }
                expanded
            }
        }
    }
}

/// Whether a built-in role id is hidden from the selector UI.
fn seen_is_hidden(role: &str) -> bool {
    ModelRole::from_id(role)
        .map(|r| builtin_role_info(r).hidden)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(pairs: &[(&str, &str)]) -> RoleRegistry {
        let mut r = RoleRegistry::new();
        for (role, model) in pairs {
            r.set(*role, *model);
        }
        r
    }

    #[test]
    fn role_str_roundtrip() {
        for role in ModelRole::ALL {
            let s = role.as_str();
            assert_eq!(ModelRole::from_id(s), Some(role));
        }
        assert_eq!(ModelRole::from_id("custom"), None);
    }

    #[test]
    fn builtin_metadata_matches_omp() {
        assert_eq!(builtin_role_info(ModelRole::Smol).tag, Some("SMOL"));
        assert!(builtin_role_info(ModelRole::Title).hidden);
        assert_eq!(builtin_role_info(ModelRole::Commit).color, RoleColor::Dim);
        assert!(!builtin_visible_ids().contains(&"title"));
        assert!(builtin_visible_ids().contains(&"commit"));
        assert_eq!(ModelRole::ALL.len(), 10);
    }

    #[test]
    fn concrete_pattern_passes_through() {
        let r = registry(&[("default", "anthropic/claude-sonnet-4")]);
        assert_eq!(r.resolve("default"), vec!["anthropic/claude-sonnet-4"]);
    }

    #[test]
    fn non_alias_pi_prefix_is_concrete() {
        // pi/<custom> is NOT an alias (custom roles are concrete), so it stays literal.
        let r = registry(&[("default", "pi/mygateway/model")]);
        assert_eq!(r.resolve("default"), vec!["pi/mygateway/model"]);
    }

    #[test]
    fn cross_role_alias_expands() {
        // default -> pi/slow, slow -> anthropic/claude-opus
        let r = registry(&[("default", "pi/slow"), ("slow", "anthropic/claude-opus")]);
        assert_eq!(r.resolve("default"), vec!["anthropic/claude-opus"]);
    }

    #[test]
    fn alias_preserves_thinking_suffix() {
        let r = registry(&[
            ("default", "pi/slow:high"),
            ("slow", "anthropic/claude-opus"),
        ]);
        assert_eq!(r.resolve("default"), vec!["anthropic/claude-opus:high"]);
    }

    #[test]
    fn cycle_terminates() {
        // a -> pi/b, b -> pi/a  (roles "smol"/"slow" used as the cycle pair)
        let r = registry(&[("smol", "pi/slow"), ("slow", "pi/smol")]);
        // Resolving smol: visits smol, expands to pi/slow -> visits slow,
        // expands to pi/smol -> already visited -> empty. Net: nothing resolves.
        assert!(r.resolve("smol").is_empty());
    }

    #[test]
    fn self_alias_collapses_to_builtin_defaults() {
        // default = "pi/default" -> self-alias -> builtin defaults (empty in oxicode)
        let r = registry(&[("default", "pi/default")]);
        assert!(r.resolve("default").is_empty());
    }

    #[test]
    fn unset_smol_inherits_default() {
        let r = registry(&[("default", "anthropic/claude-sonnet-4")]);
        // smol is unset but inherits default.
        assert_eq!(r.resolve("smol"), vec!["anthropic/claude-sonnet-4"]);
    }

    #[test]
    fn unset_non_inheriting_role_resolves_empty() {
        // commit does NOT inherit default; unset -> empty.
        let r = registry(&[("default", "anthropic/claude-sonnet-4")]);
        assert!(r.resolve("commit").is_empty());
    }

    #[test]
    fn smol_self_alias_via_default_collapses() {
        // default = "pi/smol"; resolving smol (unset) inherits default, which
        // aliases back to smol -> self-alias -> builtin (empty).
        let r = registry(&[("default", "pi/smol")]);
        assert!(r.resolve("smol").is_empty());
    }

    #[test]
    fn comma_list_normalizes() {
        let r = registry(&[("default", "openai/gpt-4o, anthropic/claude-haiku")]);
        assert_eq!(
            r.resolve("default"),
            vec!["openai/gpt-4o", "anthropic/claude-haiku"]
        );
    }

    #[test]
    fn openrouter_variant_suffix_not_split() {
        // :nitro is a routing variant, not a thinking level -> stays attached.
        let r = registry(&[("default", "openrouter/anthropic/claude-haiku:nitro")]);
        assert_eq!(
            r.resolve("default"),
            vec!["openrouter/anthropic/claude-haiku:nitro"]
        );
    }

    #[test]
    fn custom_role_accepted() {
        let r = registry(&[("myrole", "google/gemini-2.5-flash")]);
        assert_eq!(r.get("myrole"), Some("google/gemini-2.5-flash"));
        assert_eq!(r.resolve("myrole"), vec!["google/gemini-2.5-flash"]);
    }

    #[test]
    fn known_ids_builtins_then_customs_sorted() {
        let mut r = registry(&[("zebra", "a/b"), ("default", "c/d")]);
        r.set("alpha", "e/f");
        let ids = r.known_ids();
        // built-in visible first (default in declaration order), then customs sorted.
        assert_eq!(ids.first(), Some(&"default".to_string()));
        let custom_start = ids
            .iter()
            .position(|x| x == "alpha")
            .expect("alpha present");
        assert!(ids[custom_start..].contains(&"zebra".to_string()));
        assert!(custom_start < ids.iter().position(|x| x == "zebra").unwrap());
        // hidden built-in 'title' is excluded.
        assert!(!ids.contains(&"title".to_string()));
    }

    #[test]
    fn resolve_unset_and_unconfigured_default_is_empty() {
        let r = RoleRegistry::new();
        assert!(r.resolve("default").is_empty());
        assert!(r.resolve("smol").is_empty());
    }
}
