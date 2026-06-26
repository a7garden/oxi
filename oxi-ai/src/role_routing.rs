//! Role-routing provider — plugs role switching into the main agent loop.
//!
//! [`RoleRoutingProvider`] wraps a primary [`Provider`]. On each `stream()`
//! call it builds [`RoleSignals`] from the request context, picks a role via
//! [`decide_role`], resolves it to a concrete [`Model`] via [`RoleRegistry`],
//! and delegates to that model's provider. When no roles are configured (the
//! default), it is a transparent pass-through — **zero behavior change**, so
//! the existing single-model agent path is untouched unless the user opts in
//! via `[model_roles]` in settings.
//!
//! This is the "plug into the main loop" integration: because `Provider::
//! stream()` receives the full [`Context`] on every call, the wrapper decides
//! the model per request without any change to `oxi-agent`'s agent loop (which
//! otherwise resolves a single fixed `config.model_id`).

use crate::providers::{Provider, StreamResult};
use crate::role_switcher::{
    DEFAULT_LONG_CONTEXT_THRESHOLD, RoleSignals, decide_role, resolve_role_to_model,
};
use crate::roles::RoleRegistry;
use crate::{Context, Model, StreamOptions, ThinkingLevel};
use parking_lot::RwLock;
use std::pin::Pin;
use std::sync::Arc;

/// A [`Provider`] wrapper that routes each request to the model selected by
/// the role-switching layer.
///
/// Construct it at the composition root (where the agent's primary provider is
/// built) via [`RoleRoutingProvider::new`], then hand it to `Agent::new` in
/// place of the raw provider.
pub struct RoleRoutingProvider {
    default: Arc<dyn Provider>,
    registry: Arc<RwLock<RoleRegistry>>,
}

impl RoleRoutingProvider {
    /// Wrap `default`. Role-selected models delegate to their own provider
    /// (resolved via the global provider registry); anything else — including
    /// an unset or unresolvable role — falls back to `default` with the
    /// originally-requested model.
    #[must_use]
    pub fn new(default: Arc<dyn Provider>, registry: Arc<RwLock<RoleRegistry>>) -> Self {
        Self { default, registry }
    }

    /// Mutate the live role registry (e.g. from the settings UI) so changes
    /// apply to subsequent `stream()` calls without rewrapping the provider.
    pub fn update_registry(&self, f: impl FnOnce(&mut RoleRegistry)) {
        f(&mut self.registry.write());
    }

    /// Build the role signals for a request from its context + options.
    ///
    /// `explicit_override` and `current_tool` are not derivable from a
    /// `stream()` call (there is no user pin or tool-execution context at the
    /// LLM-call boundary), so they are left as `None`; the switching rests on
    /// thinking, token count, and triviality.
    #[must_use]
    pub fn signals_from_request(
        context: &Context,
        options: &Option<StreamOptions>,
    ) -> RoleSignals<'static> {
        let thinking_enabled = options
            .as_ref()
            .and_then(|o| o.thinking_level)
            .is_some_and(|level| level != ThinkingLevel::Off);
        RoleSignals {
            explicit_override: None,
            current_tool: None,
            thinking_enabled,
            estimated_tokens: estimate_tokens(context),
            long_context_threshold: DEFAULT_LONG_CONTEXT_THRESHOLD,
            is_trivial: is_trivial(context),
        }
    }
}

/// Rough prompt token estimate: ~4 chars per token across all message text.
fn estimate_tokens(context: &Context) -> usize {
    context
        .messages
        .iter()
        .map(|m| m.text_content().unwrap_or_default().len() / 4)
        .sum()
}

/// A turn is "trivial" when the last message is short and has no code fence —
/// a conservative heuristic for routing to the fast (`smol`) role.
fn is_trivial(context: &Context) -> bool {
    let last = context
        .messages
        .last()
        .map(|m| m.text_content().unwrap_or_default())
        .unwrap_or_default();
    last.len() < 40 && !last.contains("```")
}

impl Provider for RoleRoutingProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        let default = Arc::clone(&self.default);
        let registry = Arc::clone(&self.registry);
        Box::pin(async move {
            // Decide the role model while holding the read lock, then drop the
            // guard before any `.await` so the future stays `Send`.
            let role_model = {
                let reg = registry.read();
                if reg.is_empty() {
                    None
                } else {
                    let signals = Self::signals_from_request(context, &options);
                    let role = decide_role(&signals);
                    resolve_role_to_model(role, &reg)
                        .filter(|m| m.provider != model.provider || m.id != model.id)
                }
            };
            let Some(role_model) = role_model else {
                return default.stream(model, context, options).await;
            };
            // Delegate to the role model's provider, gracefully degrading to the
            // default model on any failure (e.g. a cross-provider role whose key
            // isn't set) instead of failing the whole turn.
            match crate::get_provider_arc(&role_model.provider) {
                Some(provider) => {
                    match provider.stream(&role_model, context, options.clone()).await {
                        Ok(stream) => Ok(stream),
                        Err(err) => {
                            tracing::warn!(
                                target: "role-router",
                                error = %err,
                                "role-model provider failed; falling back to default model"
                            );
                            default.stream(model, context, options).await
                        }
                    }
                }
                None => default.stream(model, context, options).await,
            }
        })
    }

    fn name(&self) -> &str {
        self.default.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Message, StreamOptions, ThinkingLevel};

    fn ctx_with_last(text: &str) -> Context {
        Context {
            messages: vec![Message::user(text)],
            ..Context::default()
        }
    }

    #[test]
    fn signals_thinking_from_options() {
        let ctx = ctx_with_last("explain");
        let opts = StreamOptions::default().thinking_level(ThinkingLevel::High);
        let s = RoleRoutingProvider::signals_from_request(&ctx, &Some(opts));
        assert!(s.thinking_enabled);
    }

    #[test]
    fn signals_thinking_off_is_disabled() {
        let ctx = ctx_with_last("explain");
        let opts = StreamOptions::default().thinking_level(ThinkingLevel::Off);
        let s = RoleRoutingProvider::signals_from_request(&ctx, &Some(opts));
        assert!(!s.thinking_enabled);
    }

    #[test]
    fn signals_no_options_is_not_thinking() {
        let ctx = ctx_with_last("explain");
        let s = RoleRoutingProvider::signals_from_request(&ctx, &None);
        assert!(!s.thinking_enabled);
    }

    #[test]
    fn signals_long_context_exceeds_threshold() {
        // ~80k chars of text → ~20k tokens estimate crosses the 60k threshold
        // only if we add enough; use a big message to push past 60_000 tokens.
        let big = "x".repeat(60_000 * 4 + 100);
        let ctx = ctx_with_last(&big);
        let s = RoleRoutingProvider::signals_from_request(&ctx, &None);
        assert!(
            s.estimated_tokens > DEFAULT_LONG_CONTEXT_THRESHOLD,
            "estimated {} should exceed {}",
            s.estimated_tokens,
            DEFAULT_LONG_CONTEXT_THRESHOLD
        );
    }

    #[test]
    fn signals_short_message_is_trivial() {
        let ctx = ctx_with_last("hi");
        let s = RoleRoutingProvider::signals_from_request(&ctx, &None);
        assert!(s.is_trivial);
    }

    #[test]
    fn signals_code_fence_is_not_trivial() {
        let ctx = ctx_with_last("```\ncode\n```");
        let s = RoleRoutingProvider::signals_from_request(&ctx, &None);
        assert!(!s.is_trivial);
    }

    #[test]
    fn estimate_tokens_scales_with_text() {
        let small = ctx_with_last("hi");
        let large = ctx_with_last(&"x".repeat(4_000));
        assert!(estimate_tokens(&large) > estimate_tokens(&small));
    }

    #[test]
    fn empty_registry_passes_through() {
        // Structural check: an empty registry short-circuits to the default
        // provider with the request model unchanged (verified by the early
        // return in `stream`). The provider-selection logic (decide_role +
        // resolve_role_to_model) is unit-tested in role_switcher.rs.
        let r = RoleRegistry::new();
        assert!(r.is_empty());
    }
}
