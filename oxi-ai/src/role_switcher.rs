//! Role-switching decision engine — the "switching layer" on top of the
//! [`crate::roles`] registry.
//!
//! Given observable turn signals, [`decide_role`] picks the active
//! [`ModelRole`], and [`resolve_role_to_model`] turns the role into a concrete
//! [`crate::Model`] via [`RoleRegistry`] + the model registry.
//!
//! Inspired by claude-code-router's signal-based `getUseModel` priority
//! (router.ts:124-200): the decision rests on *observable* signals (explicit
//! override, the tool currently running, the thinking flag, a token-count
//! threshold, triviality) — not on keyword-guessing the user's intent. The
//! role→model mapping itself is omp's role model (see [`crate::roles`]).
//!
//! This module is deliberately pure and side-effect-free; wiring it into the
//! live agent loop / a specific tool is the consumer's job.

use crate::Model;
use crate::roles::{ModelRole, RoleRegistry};

/// Default token count above which a turn routes to the long-context role.
///
/// Matches claude-code-router's default `longContextThreshold`.
pub const DEFAULT_LONG_CONTEXT_THRESHOLD: usize = 60_000;

/// Observable signals for a turn, used to decide the active role.
///
/// All fields default sanely via [`RoleSignals::default`]; set only what the
/// caller actually knows.
#[derive(Debug, Clone)]
pub struct RoleSignals<'a> {
    /// Explicit user override (e.g. a `/model` pin). Highest priority.
    pub explicit_override: Option<ModelRole>,
    /// Name of the tool currently executing, if any. A tool may declare a role
    /// (see [`role_for_tool`]).
    pub current_tool: Option<&'a str>,
    /// Whether extended thinking is enabled for the turn.
    pub thinking_enabled: bool,
    /// Estimated prompt token count for the turn.
    pub estimated_tokens: usize,
    /// Token threshold above which the long-context role is selected.
    pub long_context_threshold: usize,
    /// Whether the turn is trivially simple (short, single intent).
    pub is_trivial: bool,
}

impl Default for RoleSignals<'_> {
    fn default() -> Self {
        Self {
            explicit_override: None,
            current_tool: None,
            thinking_enabled: false,
            estimated_tokens: 0,
            long_context_threshold: DEFAULT_LONG_CONTEXT_THRESHOLD,
            is_trivial: false,
        }
    }
}

/// Decide the active role from signals, in priority order.
///
/// 1. [`RoleSignals::explicit_override`] — a user pin always wins.
/// 2. [`RoleSignals::current_tool`] — a tool declares its role
///    (e.g. `commit` → [`ModelRole::Commit`]).
/// 3. long context — `estimated_tokens > long_context_threshold` →
///    [`ModelRole::Slow`].
/// 4. [`RoleSignals::thinking_enabled`] → [`ModelRole::Slow`].
/// 5. [`RoleSignals::is_trivial`] → [`ModelRole::Smol`].
/// 6. otherwise → [`ModelRole::Default`].
///
/// Long-context and thinking both select `Slow` (the heavy model) but are
/// distinct signals: a long context may need a large window even without
/// extended thinking, and thinking may be on for a short prompt.
#[must_use]
pub fn decide_role(signals: &RoleSignals<'_>) -> ModelRole {
    if let Some(role) = signals.explicit_override {
        return role;
    }
    if let Some(tool) = signals.current_tool
        && let Some(role) = role_for_tool(tool)
    {
        return role;
    }
    if signals.estimated_tokens > signals.long_context_threshold {
        return ModelRole::Slow;
    }
    if signals.thinking_enabled {
        return ModelRole::Slow;
    }
    if signals.is_trivial {
        return ModelRole::Smol;
    }
    ModelRole::Default
}

/// Map a tool name to the role it should run under, if it declares one.
///
/// Currently only the `commit` tool declares a role ([`ModelRole::Commit`]).
/// Adding a tool→role binding here is the single place to extend tool-driven
/// switching.
#[must_use]
pub fn role_for_tool(tool_name: &str) -> Option<ModelRole> {
    match tool_name {
        "commit" => Some(ModelRole::Commit),
        _ => None,
    }
}

/// Resolve a role to a concrete [`Model`] via the registry + model registry.
///
/// Takes the first concrete pattern the role resolves to, splits it as
/// `provider/model`, and looks the model up. Returns `None` when the role is
/// unset, resolves to nothing, lacks a `/`, or names an unknown model.
#[must_use]
pub fn resolve_role_to_model(role: ModelRole, registry: &RoleRegistry) -> Option<Model> {
    let pattern = registry.resolve(role.as_str()).into_iter().next()?;
    let (provider, model_id) = pattern.split_once('/')?;
    crate::lookup_model(provider, model_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_over_everything() {
        let s = RoleSignals {
            explicit_override: Some(ModelRole::Advisor),
            current_tool: Some("commit"),
            thinking_enabled: true,
            estimated_tokens: 100_000,
            is_trivial: false,
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Advisor);
    }

    #[test]
    fn tool_signal_selects_commit_role() {
        let s = RoleSignals {
            current_tool: Some("commit"),
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Commit);
    }

    #[test]
    fn unknown_tool_falls_through() {
        let s = RoleSignals {
            current_tool: Some("read"),
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Default);
    }

    #[test]
    fn long_context_selects_slow() {
        let s = RoleSignals {
            estimated_tokens: 80_000,
            long_context_threshold: 60_000,
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Slow);
    }

    #[test]
    fn long_context_respects_custom_threshold() {
        let s = RoleSignals {
            estimated_tokens: 5_000,
            long_context_threshold: 4_000,
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Slow);
    }

    #[test]
    fn thinking_selects_slow_even_when_short() {
        let s = RoleSignals {
            thinking_enabled: true,
            estimated_tokens: 100,
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Slow);
    }

    #[test]
    fn trivial_selects_smol() {
        let s = RoleSignals {
            is_trivial: true,
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Smol);
    }

    #[test]
    fn default_when_no_signal() {
        assert_eq!(decide_role(&RoleSignals::default()), ModelRole::Default);
    }

    #[test]
    fn long_context_beats_thinking_order_independence() {
        // Both map to Slow; either signal alone is sufficient.
        let s = RoleSignals {
            thinking_enabled: true,
            estimated_tokens: 100_000,
            ..RoleSignals::default()
        };
        assert_eq!(decide_role(&s), ModelRole::Slow);
    }

    #[test]
    fn role_for_tool_bindings() {
        assert_eq!(role_for_tool("commit"), Some(ModelRole::Commit));
        assert_eq!(role_for_tool("generate_image"), None);
        assert_eq!(role_for_tool(""), None);
    }

    #[test]
    fn resolve_unconfigured_role_is_none() {
        let r = RoleRegistry::new();
        assert!(resolve_role_to_model(ModelRole::Commit, &r).is_none());
    }

    #[test]
    fn resolve_pattern_without_slash_is_none() {
        let mut r = RoleRegistry::new();
        r.set("commit", "just-a-bare-id");
        assert!(resolve_role_to_model(ModelRole::Commit, &r).is_none());
    }

    #[test]
    fn resolve_unknown_model_is_none() {
        let mut r = RoleRegistry::new();
        r.set("commit", "no-such-provider/does-not-exist-xyz");
        assert!(resolve_role_to_model(ModelRole::Commit, &r).is_none());
    }

    #[test]
    fn resolve_registered_model_is_some() {
        // Use a unique id to stay independent of other parallel tests.
        let model = crate::Model::new(
            "role-switcher-test-model",
            "Role Switcher Test",
            crate::Api::AnthropicMessages,
            "role-switcher-test",
            "",
        );
        crate::register_model(model);
        let mut r = RoleRegistry::new();
        r.set("commit", "role-switcher-test/role-switcher-test-model");
        let resolved = resolve_role_to_model(ModelRole::Commit, &r);
        assert!(
            resolved.is_some(),
            "registered model must resolve, got {resolved:?}"
        );
    }
}
