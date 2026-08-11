//! Browse act tool — natural-language goal → grounded `BrowserTab` action.
//!
//! Closes the layer-2 gap exposed by the v0.73.0 browsing stack. The
//! 30-action L1 driver and L3 loop exist, but the "what element + how"
//! reasoning layer was still on the calling model. `browse_act` solves
//! it by construction-injecting an [`oxicode_ai::Provider`] +
//! [`oxicode_ai::Model`] and asking the LLM to ground `{goal,
//! Observation}` against a deterministic top-N candidate tier.
//!
//! Pipeline:
//! 1. Open a fresh tab, `goto(url)`, `wait_for_condition(Load)`.
//! 2. `tab.observe()` → [`Observation`].
//! 3. Deterministic Jaccard scorer produces a top-N (default 20) of
//!    interactive visible elements.
//! 4. Call the LLM with `{goal, url, title, candidates}` and parse
//!    `{ref_id, action, value, reason}` from its reply.
//! 5. Resolve `ref_id` → selector, dispatch the matched `BrowserTab`
//!    method.
//!
//! Fallback: if the provider errors (network down, model missing, etc.),
//! the tool degrades to the deterministic top-1 scorer and surfaces
//! `mode: "deterministic_fallback"` in the result. It never silently
//! fails.

use super::config::BrowseConfig;
use super::engine::{BrowserEngine, BrowserError, Observation, ObservedElement};
use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::oneshot;

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "on", "in", "of", "to", "and", "or", "for", "with", "click", "tap", "press",
];

const ROLE_PRIORITY: &[&str] = &[
    "button", "link", "textbox", "checkbox", "combobox", "menuitem", "tab", "generic",
];

const DEFAULT_CANDIDATE_TOP_N: usize = 20;

// ── Candidate tier (deterministic) ─────────────────────────────────────────

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty() && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

fn role_rank(role: &str) -> usize {
    ROLE_PRIORITY
        .iter()
        .position(|r| *r == role)
        .unwrap_or(ROLE_PRIORITY.len())
}

/// Build the top-N candidate list to send to the LLM.
///
/// Filters to visible + interactive; ranks by Jaccard token overlap with
/// the goal; tie-breaks on role priority, name length, DOM order.
pub fn candidate_tier(goal: &str, obs: &Observation, top_n: usize) -> Vec<ObservedElement> {
    let goal_tokens = tokenize(goal);
    if goal_tokens.is_empty() {
        return Vec::new();
    }
    let dom_index: std::collections::HashMap<&str, usize> = obs
        .elements
        .iter()
        .enumerate()
        .map(|(i, e)| (e.ref_id.as_str(), i))
        .collect();
    let mut scored: Vec<(f64, &ObservedElement)> = obs
        .elements
        .iter()
        .filter(|e| e.visible && e.interactive)
        .filter_map(|e| {
            let hay = tokenize(&format!("{} {}", e.name, e.role));
            if hay.is_empty() {
                return None;
            }
            let matched = goal_tokens.iter().filter(|t| hay.contains(t)).count();
            if matched == 0 {
                return None;
            }
            Some((matched as f64 / goal_tokens.len() as f64, e))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(role_rank(&a.1.role).cmp(&role_rank(&b.1.role)))
            .then(b.1.name.len().cmp(&a.1.name.len()))
            .then(
                dom_index
                    .get(a.1.ref_id.as_str())
                    .unwrap_or(&0)
                    .cmp(dom_index.get(b.1.ref_id.as_str()).unwrap_or(&0)),
            )
    });
    scored
        .into_iter()
        .take(top_n)
        .map(|(_, e)| e.clone())
        .collect()
}

// ── Action enum ───────────────────────────────────────────────────────────

/// Action the LLM picks for the matched element. Maps 1:1 to a
/// [`BrowserTab`](super::BrowserTab) method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserActAction {
    /// Dispatch `BrowserTab::click(selector)`.
    Click,
    /// Dispatch `BrowserTab::type_(selector, value)`.
    Type,
    /// Dispatch `BrowserTab::fill(selector, value)`.
    Fill,
    /// Dispatch `BrowserTab::select_option(selector, value)`.
    SelectOption,
    /// Dispatch `BrowserTab::check(selector)`.
    Check,
    /// Dispatch `BrowserTab::uncheck(selector)`.
    Uncheck,
    /// Dispatch `BrowserTab::press(value)`. Selector is ignored.
    Press,
    /// Dispatch `BrowserTab::hover(selector)`.
    Hover,
}

impl BrowserActAction {
    /// Parse the action string the LLM returned.
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "click" => Some(Self::Click),
            "type" => Some(Self::Type),
            "fill" => Some(Self::Fill),
            "select_option" => Some(Self::SelectOption),
            "check" => Some(Self::Check),
            "uncheck" => Some(Self::Uncheck),
            "press" => Some(Self::Press),
            "hover" => Some(Self::Hover),
            _ => None,
        }
    }

    /// Whether the action needs a `value` (type/fill/select_option/press).
    pub fn requires_value(self) -> bool {
        matches!(
            self,
            Self::Type | Self::Fill | Self::SelectOption | Self::Press
        )
    }

    /// Stable string form used in tool results and JSON serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Type => "type",
            Self::Fill => "fill",
            Self::SelectOption => "select_option",
            Self::Check => "check",
            Self::Uncheck => "uncheck",
            Self::Press => "press",
            Self::Hover => "hover",
        }
    }
}

/// Parsed LLM grounding reply.
#[derive(Debug, Clone)]
pub struct LLMGroundResult {
    /// The `ref_id` the LLM picked, or `None` when the LLM declared no match.
    pub ref_id: Option<String>,
    /// The action to dispatch against the matched element.
    pub action: BrowserActAction,
    /// Value for type/fill/select_option/press; `None` when not required.
    pub value: Option<String>,
    /// The LLM's free-text reason. Surfaced in the tool result payload.
    pub reason: Option<String>,
}

// ── LLM grounding ─────────────────────────────────────────────────────────

/// Call the LLM with `{goal, candidates, observation}` and parse its
/// JSON reply. Returns [`BrowserError::GroundingParse`] if the response
/// isn't parseable, or [`BrowserError::Backend`] if the provider errors.
pub async fn llm_ground(
    provider: &dyn oxicode_ai::Provider,
    model: &oxicode_ai::Model,
    goal: &str,
    candidates: &[ObservedElement],
    observation: &Observation,
) -> Result<LLMGroundResult, BrowserError> {
    let prompt = format!(
        "GOAL: {goal}\nURL: {url}\nTITLE: {title}\n\n\
         CANDIDATE ELEMENTS:\n{cands}\n\n\
         TASK: pick the single element that best matches the goal.\n\
         Return JSON: {{\"ref_id\": \"eN\", \"action\": \"click|type|fill|select_option|check|uncheck|press|hover\", \"value\": \"...\", \"reason\": \"...\"}}\n\
         If none match, return: {{\"ref_id\": null, \"reason\": \"...\"}}",
        goal = goal,
        url = observation.url,
        title = observation.title,
        cands = serde_json::to_string(candidates).unwrap_or_default(),
    );

    let mut ctx = oxicode_ai::Context::new();
    ctx.add_message(oxicode_ai::Message::user(prompt));

    let stream = provider
        .stream(model, &ctx, None)
        .await
        .map_err(|e| BrowserError::Backend(e.to_string()))?;

    let mut pinned: std::pin::Pin<
        Box<dyn futures::Stream<Item = oxicode_ai::ProviderEvent> + Send>,
    > = stream;
    let mut text = String::new();
    while let Some(ev) = pinned.next().await {
        match ev {
            oxicode_ai::ProviderEvent::TextDelta { delta, .. } => text.push_str(&delta),
            oxicode_ai::ProviderEvent::Done { .. } => break,
            oxicode_ai::ProviderEvent::Error { error, .. } => {
                let msg = error
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "LLM stream returned error event".into());
                return Err(BrowserError::Backend(msg));
            }
            _ => {}
        }
    }

    // Strip optional ```json fences.
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let v: Value = serde_json::from_str(trimmed)
        .map_err(|e| BrowserError::GroundingParse(format!("{e}: {trimmed}")))?;
    let ref_id = v.get("ref_id").and_then(|x| x.as_str()).map(String::from);
    let action_str = v.get("action").and_then(|x| x.as_str()).unwrap_or("click");
    let action = BrowserActAction::parse_str(action_str)
        .ok_or_else(|| BrowserError::GroundingParse(format!("unknown action: {action_str}")))?;
    let value = v.get("value").and_then(|x| x.as_str()).map(String::from);
    let reason = v.get("reason").and_then(|x| x.as_str()).map(String::from);
    Ok(LLMGroundResult {
        ref_id,
        action,
        value,
        reason,
    })
}

// ── Tool ───────────────────────────────────────────────────────────────────

/// Browse act tool — natural-language goal → grounded action.
///
/// The LLM is construction-injected. The tool opens a tab, observes the
/// page, asks the LLM to pick an element + action from a top-N candidate
/// tier, and dispatches the matched `BrowserTab` method.
pub struct BrowseActTool {
    /// Browser engine that owns the tab lifecycle.
    engine: Arc<dyn BrowserEngine>,
    /// Tool-level configuration (timeouts, etc.).
    config: BrowseConfig,
    /// Reasoning capability, used to ground `goal` against `Observation`.
    /// `None` means deterministic-only mode.
    provider: Option<Arc<dyn oxicode_ai::Provider>>,
    /// Model identifier for the provider. `None` means deterministic-only mode.
    model: Option<oxicode_ai::Model>,
    /// Shared callback management (progress + browse progress).
    callbacks: super::callback_mixin::BrowseCallbacks,
    /// Shared slot for the current tab's ID.
    tab_id_slot: Mutex<Arc<parking_lot::Mutex<Option<uuid::Uuid>>>>,
}

impl BrowseActTool {
    /// Create with the default config and a real LLM (the normal path).
    pub fn new(
        provider: Arc<dyn oxicode_ai::Provider>,
        model: oxicode_ai::Model,
        engine: Arc<dyn BrowserEngine>,
    ) -> Self {
        Self::with_config(provider, model, engine, BrowseConfig::default())
    }

    /// Create with a custom config and a real LLM.
    pub fn with_config(
        provider: Arc<dyn oxicode_ai::Provider>,
        model: oxicode_ai::Model,
        engine: Arc<dyn BrowserEngine>,
        config: BrowseConfig,
    ) -> Self {
        Self {
            engine,
            config,
            provider: Some(provider),
            model: Some(model),
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: Mutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }

    /// Create with the default config and no LLM (deterministic-only mode).
    ///
    /// Every `execute` call returns `mode: "deterministic_fallback"`.
    /// Used by callers that can't wire an LLM (offline tests, MCP servers
    /// without model access).
    pub fn new_deterministic(engine: Arc<dyn BrowserEngine>) -> Self {
        Self::with_config_deterministic(engine, BrowseConfig::default())
    }

    /// Create with a custom config and no LLM (deterministic-only mode).
    pub fn with_config_deterministic(engine: Arc<dyn BrowserEngine>, config: BrowseConfig) -> Self {
        Self {
            engine,
            config,
            provider: None,
            model: None,
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: Mutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }
}

#[async_trait]
impl AgentTool for BrowseActTool {
    fn name(&self) -> &str {
        "browse_act"
    }

    fn label(&self) -> &str {
        "Browse Act"
    }

    fn description(&self) -> &str {
        "Act on a web page using a natural-language goal. The tool observes the page's \
         interactive surface, calls a model to pick the element matching your goal, and \
         dispatches the right click/type/fill/select_option/check/uncheck/press/hover. \
         No CSS selectors required from the caller. Use when you know what you want to do \
         but not which element to target."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL of the page to act on"
                },
                "goal": {
                    "type": "string",
                    "description": "Natural-language action description"
                },
                "value": {
                    "type": "string",
                    "description": "Value for type/fill/select_option/press"
                },
                "action_hint": {
                    "type": "string",
                    "enum": ["click", "type", "fill", "select_option", "check", "uncheck", "press", "hover"],
                    "description": "Optional action to bias the model's choice"
                },
                "timeout": {
                    "type": "integer",
                    "default": 30,
                    "description": "Seconds"
                }
            },
            "required": ["url", "goal"]
        })
    }

    fn on_progress(&self, callback: crate::tools::ProgressCallback) {
        self.callbacks.store_progress(callback);
    }

    fn on_browse_progress(&self, callback: Arc<dyn Fn(super::BrowseProgress) + Send + Sync>) {
        self.callbacks.store_browse(callback);
    }

    fn set_tab_id_slot(&self, slot: Arc<parking_lot::Mutex<Option<uuid::Uuid>>>) {
        *self.tab_id_slot.lock() = slot;
    }

    fn current_tab_id(&self) -> Option<uuid::Uuid> {
        *self.tab_id_slot.lock().lock()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let url = params["url"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: url".to_string())?;
        let goal = params["goal"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: goal".to_string())?;
        let value = params["value"].as_str();
        let action_hint = params["action_hint"]
            .as_str()
            .and_then(BrowserActAction::parse_str);
        let timeout_secs = params["timeout"]
            .as_u64()
            .unwrap_or(self.config.page_timeout_secs);

        // 1. Open a fresh tab.
        let raw_tab = self
            .engine
            .new_tab()
            .await
            .map_err(|e| format!("Failed to open browser tab: {e}"))?;
        let tab_id = raw_tab.tab_id();
        *self.tab_id_slot.lock().lock() = Some(tab_id);
        self.callbacks
            .register_on_registry(tab_id, self.engine.callback_registry().as_ref());
        let guard = super::tab_guard::TabGuard::new(raw_tab);

        // 2. Navigate.
        let page = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            guard.tab().goto(url),
        )
        .await
        .map_err(|_| format!("Navigation timed out after {timeout_secs}s"))?
        .map_err(|e| format!("Navigation failed: {e}"))?;

        // 3. Observe.
        let observation = guard
            .tab()
            .observe()
            .await
            .map_err(|e| format!("Observation failed: {e}"))?;

        // 4. Candidate tier.
        let candidates = candidate_tier(goal, &observation, DEFAULT_CANDIDATE_TOP_N);

        // 5. LLM grounding (with deterministic fallback).
        let mut mode = "llm";
        let mut ground_result = match (self.provider.as_ref(), self.model.as_ref()) {
            (Some(p), Some(m)) => llm_ground(p.as_ref(), m, goal, &candidates, &observation).await,
            _ => {
                // Deterministic-only mode: skip LLM, top scorer wins.
                mode = "deterministic_only";
                let action = action_hint.unwrap_or(BrowserActAction::Click);
                match candidates.first() {
                    Some(top) => Ok(LLMGroundResult {
                        ref_id: Some(top.ref_id.clone()),
                        action,
                        value: value.map(String::from),
                        reason: Some("deterministic-only mode (no LLM wired)".into()),
                    }),
                    None => Err(BrowserError::NoMatch(
                        "no candidates on page (deterministic-only mode)".into(),
                    )),
                }
            }
        };

        if matches!(ground_result, Err(BrowserError::Backend(_))) {
            // Top scorer wins deterministically when the provider is down.
            if let Some(top) = candidates.first() {
                let action = action_hint.unwrap_or(BrowserActAction::Click);
                let fallback_reason = match &ground_result {
                    Err(BrowserError::Backend(s)) => format!("provider error: {s}"),
                    _ => "provider unavailable".into(),
                };
                ground_result = Ok(LLMGroundResult {
                    ref_id: Some(top.ref_id.clone()),
                    action,
                    value: value.map(String::from),
                    reason: Some(format!("deterministic fallback ({fallback_reason})")),
                });
                mode = "deterministic_fallback";
            }
        }
        let ground = ground_result.map_err(|e| e.to_string())?;

        // 6. No match → return result without dispatching.
        if ground.ref_id.is_none() {
            let body = json!({
                "matched_ref": Value::Null,
                "matched_name": Value::Null,
                "matched_role": Value::Null,
                "action": Value::Null,
                "selector": Value::Null,
                "score": 0.0,
                "result": "no_match",
                "mode": mode,
                "reason": ground.reason,
                "candidates_considered": candidates.len(),
            });
            guard.close().await;
            *self.tab_id_slot.lock().lock() = None;
            return Ok(AgentToolResult::success(body.to_string()));
        }

        // 7. Resolve selector from candidates or observation.
        let ref_id_str = ground.ref_id.as_deref().ok_or_else(|| {
            "LLM produced null ref_id but did not return no-match error".to_string()
        })?;
        let el = candidates
            .iter()
            .find(|e| e.ref_id == ref_id_str)
            .cloned()
            .or_else(|| {
                observation
                    .elements
                    .iter()
                    .find(|e| e.ref_id == ref_id_str)
                    .cloned()
            })
            .ok_or_else(|| {
                format!("LLM picked ref_id={ref_id_str} but it is not in the observation")
            })?;
        let selector = el.selector.clone();
        let matched_name = el.name.clone();
        let matched_role = el.role.clone();

        // 8. Resolve final action + value (hint overrides LLM action).
        let action = action_hint.unwrap_or(ground.action);
        let final_value = ground.value.as_deref().or(value);

        // 9. Dispatch.
        if action.requires_value() && final_value.map(str::is_empty).unwrap_or(true) {
            return Err(BrowserError::MissingValue {
                action: action.as_str(),
            }
            .into());
        }
        let v = final_value.unwrap_or("");
        match action {
            BrowserActAction::Click => guard.tab().click(&selector).await,
            BrowserActAction::Hover => guard.tab().hover(&selector).await,
            BrowserActAction::Check => guard.tab().check(&selector).await,
            BrowserActAction::Uncheck => guard.tab().uncheck(&selector).await,
            BrowserActAction::Type => guard.tab().type_(&selector, v).await,
            BrowserActAction::Fill => guard.tab().fill(&selector, v).await,
            BrowserActAction::SelectOption => guard.tab().select_option(&selector, v).await,
            BrowserActAction::Press => guard.tab().press(v).await,
        }
        .map_err(|e| e.to_string())?;

        guard.close().await;
        *self.tab_id_slot.lock().lock() = None;

        let body = json!({
            "matched_ref": ref_id_str,
            "matched_name": matched_name,
            "matched_role": matched_role,
            "action": action.as_str(),
            "selector": selector,
            "score": 0.0,
            "result": "ok",
            "mode": mode,
            "reason": ground.reason,
            "candidates_considered": candidates.len(),
        });
        Ok(AgentToolResult::success(body.to_string())
            .with_metadata(json!({ "url": page.url, "title": page.title })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browse::engine::{Observation, ObservedElement};

    fn el(
        ref_id: &str,
        role: &str,
        name: &str,
        visible: bool,
        interactive: bool,
    ) -> ObservedElement {
        ObservedElement {
            ref_id: ref_id.into(),
            role: role.into(),
            name: name.into(),
            tag: "button".into(),
            selector: format!("[data-oxicode-ref=\"{ref_id}\"]"),
            visible,
            interactive,
        }
    }

    fn obs(elements: Vec<ObservedElement>) -> Observation {
        Observation {
            url: "https://example.com".into(),
            title: "ex".into(),
            elements,
        }
    }

    // ── candidate tier ─────────────────────────────────────────────

    #[test]
    fn candidate_tier_picks_highest_token_overlap() {
        let o = obs(vec![
            el("e1", "link", "Documentation", true, true),
            el("e2", "button", "Sign Up", true, true),
            el("e3", "link", "About", true, true),
        ]);
        let c = candidate_tier("click the Sign Up button", &o, 5);
        assert_eq!(c[0].ref_id, "e2");
    }

    #[test]
    fn candidate_tier_filters_hidden_elements() {
        let o = obs(vec![
            el("e1", "button", "Sign Up", false, true),
            el("e2", "button", "Cancel", true, true),
        ]);
        let c = candidate_tier("Sign Up", &o, 5);
        assert!(c.iter().all(|x| x.ref_id != "e1"));
    }

    #[test]
    fn candidate_tier_filters_non_interactive() {
        let o = obs(vec![el("e1", "text", "Sign Up Here", true, false)]);
        let c = candidate_tier("Sign Up", &o, 5);
        assert!(c.is_empty());
    }

    #[test]
    fn candidate_tier_prefers_button_over_link_on_ties() {
        let o = obs(vec![
            el("e1", "link", "Submit", true, true),
            el("e2", "button", "Submit", true, true),
        ]);
        let c = candidate_tier("Submit", &o, 5);
        assert_eq!(c[0].role, "button");
    }

    #[test]
    fn candidate_tier_prefers_longer_name_on_ties() {
        let o = obs(vec![
            el("e1", "button", "Add", true, true),
            el("e2", "button", "Add to Cart", true, true),
        ]);
        let c = candidate_tier("Add", &o, 5);
        assert_eq!(c[0].name, "Add to Cart");
    }

    #[test]
    fn candidate_tier_respects_top_n() {
        let elements: Vec<_> = (0..30)
            .map(|i| el(&format!("e{i}"), "button", &format!("Item {i}"), true, true))
            .collect();
        let o = obs(elements);
        let c = candidate_tier("Item", &o, 5);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn candidate_tier_empty_goal_yields_empty() {
        let o = obs(vec![el("e1", "button", "OK", true, true)]);
        assert!(candidate_tier("", &o, 5).is_empty());
        assert!(candidate_tier("   ", &o, 5).is_empty());
    }

    #[test]
    fn candidate_tier_drop_stop_words() {
        let o = obs(vec![el("e1", "button", "OK", true, true)]);
        let c = candidate_tier("click the OK button", &o, 5);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].ref_id, "e1");
    }

    // ── action enum ────────────────────────────────────────────────

    #[test]
    fn action_from_str_maps_known_values() {
        assert_eq!(
            BrowserActAction::parse_str("click"),
            Some(BrowserActAction::Click)
        );
        assert_eq!(
            BrowserActAction::parse_str("type"),
            Some(BrowserActAction::Type)
        );
        assert_eq!(
            BrowserActAction::parse_str("fill"),
            Some(BrowserActAction::Fill)
        );
        assert_eq!(
            BrowserActAction::parse_str("select_option"),
            Some(BrowserActAction::SelectOption)
        );
        assert_eq!(
            BrowserActAction::parse_str("check"),
            Some(BrowserActAction::Check)
        );
        assert_eq!(
            BrowserActAction::parse_str("uncheck"),
            Some(BrowserActAction::Uncheck)
        );
        assert_eq!(
            BrowserActAction::parse_str("press"),
            Some(BrowserActAction::Press)
        );
        assert_eq!(
            BrowserActAction::parse_str("hover"),
            Some(BrowserActAction::Hover)
        );
        assert_eq!(BrowserActAction::parse_str("nonsense"), None);
    }

    #[test]
    fn action_requires_value_mapping() {
        assert!(!BrowserActAction::Click.requires_value());
        assert!(BrowserActAction::Type.requires_value());
        assert!(BrowserActAction::Fill.requires_value());
        assert!(BrowserActAction::SelectOption.requires_value());
        assert!(BrowserActAction::Press.requires_value());
        assert!(!BrowserActAction::Check.requires_value());
        assert!(!BrowserActAction::Hover.requires_value());
    }

    // ── LLM grounding ──────────────────────────────────────────────

    use async_trait::async_trait;
    use oxicode_ai::{
        Api, Context, Model, Provider, ProviderError, ProviderEvent, StreamOptions, StreamResult,
    };
    use parking_lot::Mutex;
    use std::pin::Pin;

    /// Provider that emits one `TextDelta` with a fixed payload then `Done`.
    struct ScriptedProvider {
        payload: Mutex<String>,
    }
    impl ScriptedProvider {
        fn new(payload: &str) -> Arc<Self> {
            Arc::new(Self {
                payload: Mutex::new(payload.into()),
            })
        }
    }
    #[async_trait]
    impl Provider for ScriptedProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a Context,
            _options: Option<StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            let payload = self.payload.lock().clone();
            Box::pin(async move {
                use futures::stream;
                let s = stream::iter(vec![
                    ProviderEvent::TextDelta {
                        content_index: 0,
                        delta: payload,
                        partial: Arc::new(oxicode_ai::AssistantMessage::new(
                            Api::OpenAiCompletions,
                            "test-provider",
                            "test-model",
                        )),
                    },
                    ProviderEvent::Done {
                        reason: oxicode_ai::StopReason::Stop,
                        message: oxicode_ai::AssistantMessage::new(
                            Api::OpenAiCompletions,
                            "test-provider",
                            "test-model",
                        ),
                    },
                ]);
                let stream: Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>> =
                    Box::pin(s);
                Ok(stream)
            })
        }
    }

    struct FailingProvider;
    #[async_trait]
    impl Provider for FailingProvider {
        fn stream<'a>(
            &'a self,
            _model: &'a Model,
            _context: &'a Context,
            _options: Option<StreamOptions>,
        ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
            Box::pin(async move { Err(ProviderError::NetworkError("provider down".into())) })
        }
    }

    fn test_model() -> Model {
        Model::new(
            "test-model",
            "test-model",
            Api::OpenAiCompletions,
            "openai",
            "",
        )
    }

    #[tokio::test]
    async fn llm_ground_parses_ref_id_and_action() {
        let provider: Arc<dyn Provider> =
            ScriptedProvider::new(r#"{"ref_id":"e2","action":"click","reason":"matches Sign Up"}"#);
        let candidates = vec![
            el("e1", "link", "Documentation", true, true),
            el("e2", "button", "Sign Up", true, true),
            el("e3", "link", "About", true, true),
        ];
        let o = obs(candidates.clone());
        let r = llm_ground(provider.as_ref(), &test_model(), "Sign Up", &candidates, &o)
            .await
            .expect("ok");
        assert_eq!(r.ref_id.as_deref(), Some("e2"));
        assert_eq!(r.action, BrowserActAction::Click);
        assert_eq!(r.reason.as_deref(), Some("matches Sign Up"));
    }

    #[tokio::test]
    async fn llm_ground_handles_null_ref_id() {
        let provider: Arc<dyn Provider> =
            ScriptedProvider::new(r#"{"ref_id":null,"reason":"no matching element"}"#);
        let candidates = vec![el("e1", "button", "Cancel", true, true)];
        let o = obs(candidates.clone());
        let r = llm_ground(
            provider.as_ref(),
            &test_model(),
            "Submit Order",
            &candidates,
            &o,
        )
        .await
        .expect("ok");
        assert!(r.ref_id.is_none());
        assert_eq!(r.reason.as_deref(), Some("no matching element"));
    }

    #[tokio::test]
    async fn llm_ground_returns_grounding_parse_on_invalid_json() {
        let provider: Arc<dyn Provider> = ScriptedProvider::new("not json at all");
        let candidates = vec![el("e1", "button", "OK", true, true)];
        let o = obs(candidates.clone());
        let err = llm_ground(provider.as_ref(), &test_model(), "OK", &candidates, &o)
            .await
            .expect_err("should fail to parse");
        assert!(matches!(err, BrowserError::GroundingParse(_)));
    }

    #[tokio::test]
    async fn llm_ground_propagates_provider_error() {
        let provider: Arc<dyn Provider> = Arc::new(FailingProvider);
        let candidates = vec![el("e1", "button", "OK", true, true)];
        let o = obs(candidates.clone());
        let err = llm_ground(provider.as_ref(), &test_model(), "OK", &candidates, &o)
            .await
            .expect_err("provider error");
        assert!(matches!(err, BrowserError::Backend(_)));
    }

    #[tokio::test]
    async fn llm_ground_strips_json_fences() {
        let provider: Arc<dyn Provider> =
            ScriptedProvider::new("```json\n{\"ref_id\":\"e1\",\"action\":\"click\"}\n```");
        let candidates = vec![el("e1", "button", "OK", true, true)];
        let o = obs(candidates.clone());
        let r = llm_ground(provider.as_ref(), &test_model(), "OK", &candidates, &o)
            .await
            .expect("ok");
        assert_eq!(r.ref_id.as_deref(), Some("e1"));
        assert_eq!(r.action, BrowserActAction::Click);
    }
}
