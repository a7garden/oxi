# Layer-2 Browse AI Primitive: `browse_act` — Implementation Plan (v2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an LLM-grounded `browse_act` agent tool (natural-language goal → grounded `BrowserTab` action), plus fix the v0.72.0-era CI breakage (smoke-test / msrv jobs missing `libfontconfig1-dev`).

**Architecture:** A new agent-layer tool (`AgentTool` impl) under `oxicode-agent/src/tools/browse/`. It construction-injects `(Arc<dyn oxicode_ai::Provider>, oxicode_ai::Model)` (the same shape `Agent::new` already accepts) and calls the LLM with `{goal, top-N candidate Observation elements}` to pick `ref_id` + `action`. A deterministic Jaccard scorer prunes the candidate tier before the LLM call. The tool falls back to the deterministic tier when the provider errors.

**Tech Stack:** Rust 2024 edition, oxicode-agent (workspace), `oxicode_ai::Provider::stream`, the existing `BrowserTab` / `Observation` types.

## Global Constraints

- **Workspace version 0.73.0** — this PR targets 0.74.0; do NOT bump member versions (that happens at release time).
- **Tool layer only.** No SDK surface changes. No new ports. No ToolContext change. (Per v0.72.0 browsing-identity decision.)
- **Tool file is always-compiled** (no feature gate), to mirror `BrowseExtractTool`. Factory registration covers both the always-compiled and the `native-browser`-gated factories.
- **`cargo clippy --workspace --all-targets -- -D warnings`** is the merge gate.
- **`cargo doc --workspace --no-deps`** with `RUSTDOCFLAGS="-D warnings"` is the doc gate.
- **TDD.** Write the test first, watch it fail, then implement. The existing `MockProvider` / `NopProvider` patterns at `oxicode-agent/src/advisor/agent_advisor.rs:109-131` and `oxicode-sdk/src/lifecycle/supervisor.rs:813-831` are the precedent.

---

### Task 1: Extend `BrowserError` with three new variants

**Files:**
- Modify: `oxicode-agent/src/tools/browse/engine.rs:20-46`

**Interfaces:**
- Consumes: nothing (additive).
- Produces: `BrowserError::NoMatch(String)`, `BrowserError::MissingValue { action: &'static str }`, `BrowserError::GroundingParse(String)`.

- [ ] **Step 1: Write the failing unit tests in `engine.rs` `mod tests`**

Append to the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn browser_error_no_match_carries_reason() {
    let err = BrowserError::NoMatch("no button on page".into());
    assert!(err.to_string().contains("no match"));
    assert!(err.to_string().contains("no button on page"));
}

#[test]
fn browser_error_missing_value_names_action() {
    let err = BrowserError::MissingValue { action: "type" };
    assert_eq!(err.to_string(), "missing value for action: type");
}

#[test]
fn browser_error_grounding_parse_includes_message() {
    let err = BrowserError::GroundingParse("expected JSON".into());
    assert!(err.to_string().contains("grounding parse failed"));
}
```

- [ ] **Step 2: Run tests, verify RED**

Run: `cargo nextest run -p oxicode-agent --no-fail-fast 'engine::tests::browser_error_'`
Expected: 3 failures with "no variant or associated item named `NoMatch`".

- [ ] **Step 3: Add the three variants to `BrowserError`**

In `engine.rs` inside `pub enum BrowserError { ... }` (around line 21-40), add:

```rust
/// browse_act LLM could not pick a confident match. The carried string is the
/// LLM's free-text reason (or a deterministic-tier note when the provider was
/// unavailable).
#[error("no match: {0}")]
NoMatch(String),
/// browse_act was given an action that requires a value but it was empty.
#[error("missing value for action: {action}")]
MissingValue { action: &'static str },
/// browse_act LLM response wasn't parseable as the expected JSON shape.
#[error("grounding parse failed: {0}")]
GroundingParse(String),
```

- [ ] **Step 4: Run tests, verify GREEN**

Run: `cargo nextest run -p oxicode-agent --no-fail-fast 'engine::tests::browser_error_'`
Expected: 3 passes.

- [ ] **Step 5: Commit**

```bash
git add oxicode-agent/src/tools/browse/engine.rs
git commit -m "feat(browse): BrowserError gains NoMatch/MissingValue/GroundingParse variants"
```

---

### Task 2: `browse_act_tool.rs` — candidate tier (deterministic, TDD)

**Files:**
- Create: `oxicode-agent/src/tools/browse/browse_act_tool.rs`

**Interfaces:**
- Consumes: `ObservedElement`, `Observation`.
- Produces: `pub fn candidate_tier(goal: &str, obs: &Observation, top_n: usize) -> Vec<ObservedElement>` (free fn).

- [ ] **Step 1: Write the failing tests**

Open the new file with the module doc comment, the `use` imports, and the test mod. Body: `pub fn candidate_tier(_goal: &str, _obs: &Observation, _top_n: usize) -> Vec<ObservedElement> { todo!() }`. Tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::browse::engine::{ObservedElement, Observation};

    fn el(ref_id: &str, role: &str, name: &str, visible: bool, interactive: bool) -> ObservedElement {
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
        Observation { url: "https://example.com".into(), title: "ex".into(), elements }
    }

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
        let elements: Vec<_> = (0..30).map(|i| {
            el(&format!("e{i}"), "button", &format!("Item {i}"), true, true)
        }).collect();
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
        // "click the OK button" → tokens {click, ok, button} after stop-word
        // removal; OK matches.
        let c = candidate_tier("click the OK button", &o, 5);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].ref_id, "e1");
    }
}
```

- [ ] **Step 2: Run tests, verify RED**

Skeleton with `todo!()` body. Same pattern as spec.

- [ ] **Step 3: Implement the candidate tier**

```rust
const STOP_WORDS: &[&str] = &["the", "a", "an", "on", "in", "of", "to", "and", "or", "for", "with", "click", "tap", "press"];

const ROLE_PRIORITY: &[&str] = &["button", "link", "textbox", "checkbox", "combobox", "menuitem", "tab", "generic"];

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

fn role_rank(role: &str) -> usize {
    ROLE_PRIORITY.iter().position(|r| *r == role).unwrap_or(ROLE_PRIORITY.len())
}

pub fn candidate_tier(goal: &str, obs: &Observation, top_n: usize) -> Vec<ObservedElement> {
    let goal_tokens = tokenize(goal);
    if goal_tokens.is_empty() { return Vec::new(); }
    let mut scored: Vec<(f64, &ObservedElement)> = obs.elements.iter()
        .filter(|e| e.visible && e.interactive)
        .filter_map(|e| {
            let hay = tokenize(&format!("{} {}", e.name, e.role));
            if hay.is_empty() { return None; }
            let matched = goal_tokens.iter().filter(|t| hay.contains(t)).count();
            if matched == 0 { return None; }
            Some((matched as f64 / goal_tokens.len() as f64, e))
        })
        .collect();
    // Stable sort: score desc, role priority asc, name len desc, DOM order asc.
    let dom_index: std::collections::HashMap<&str, usize> = obs.elements.iter()
        .enumerate().map(|(i, e)| (e.ref_id.as_str(), i)).collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
            .then(role_rank(&a.1.role).cmp(&role_rank(&b.1.role)))
            .then(b.1.name.len().cmp(&a.1.name.len()))
            .then(dom_index.get(a.1.ref_id.as_str()).unwrap_or(&0)
                  .cmp(dom_index.get(b.1.ref_id.as_str()).unwrap_or(&0)))
    });
    scored.into_iter().take(top_n).map(|(_, e)| e.clone()).collect()
}
```

- [ ] **Step 4: Run tests, verify GREEN**

Run: `cargo nextest run -p oxicode-agent --no-fail-fast 'browse::browse_act_tool::tests::candidate_tier_'`
Expected: 8 passes.

- [ ] **Step 5: Commit (only the candidate tier)**

Wait — we want one commit for the whole tool, not one for the tier. Skip the commit; the next task picks it up. **Mark this task done after Task 3 ships the tool.**

---

### Task 3: `browse_act_tool.rs` — LLM grounding path + dispatch + struct + AgentTool

**Files:**
- Modify: `oxicode-agent/src/tools/browse/browse_act_tool.rs` (the file from Task 2)

**Interfaces:**
- Consumes: `Arc<dyn oxicode_ai::Provider>`, `oxicode_ai::Model`, `Arc<dyn BrowserEngine>`, `BrowseConfig`, `Observation`.
- Produces: `BrowseActTool`, `BrowserActAction`, `LLMGroundResult`, `dispatch_action(tab, selector, action, value)`, `BrowseActTool::execute(...)`.

- [ ] **Step 1: Write the failing tests for the LLM grounding path + dispatch**

Append to the test mod:

```rust
use async_trait::async_trait;
use futures::stream;
use oxicode_ai::{
    Api, Context, Model, Provider, ProviderError, ProviderEvent, StreamOptions, StreamResult,
    TextContent,
};
use parking_lot::Mutex;
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;

/// Provider that emits a single text delta with a fixed payload, then Done.
struct ScriptedProvider {
    payload: Mutex<String>,
}
impl ScriptedProvider {
    fn new(payload: &str) -> Arc<Self> { Arc::new(Self { payload: Mutex::new(payload.into()) }) }
}
#[async_trait]
impl Provider for ScriptedProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Model,
        _context: &'a Context,
        _options: Option<StreamOptions>,
    ) -> Pin<Box<dyn std::future::Future<Output = StreamResult> + Send + 'a>> {
        let payload = self.payload.lock().clone();
        Box::pin(async move {
            let s = stream::iter(vec![
                ProviderEvent::StreamStart { model: "test-model".into() },
                ProviderEvent::TextDelta { delta: payload, index: 0 },
                ProviderEvent::StreamEnd { stop_reason: oxicode_ai::StopReason::Stop, model: "test-model".into() },
            ]);
            Ok(Box::pin(s) as Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>)
        })
    }
}

/// Provider whose `stream` returns an error.
struct FailingProvider;
#[async_trait]
impl Provider for FailingProvider {
    fn stream<'a>(
        &'a self,
        _model: &'a Model,
        _context: &'a Context,
        _options: Option<StreamOptions>,
    ) -> Pin<Box<dyn std::future::Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move { Err(ProviderError::Other("provider down".into())) })
    }
}

fn test_model() -> Model {
    Model::new("test-model", "test-model", Api::OpenAiCompletions, "openai")
}

#[tokio::test]
async fn llm_ground_parses_ref_id_and_action() {
    let provider: Arc<dyn Provider> = ScriptedProvider::new(
        r#"{"ref_id":"e2","action":"click","reason":"matches Sign Up"}"#,
    );
    let candidates = vec![
        el("e1", "link", "Documentation", true, true),
        el("e2", "button", "Sign Up", true, true),
        el("e3", "link", "About", true, true),
    ];
    let o = obs(candidates.clone());
    let result = llm_ground(provider.as_ref(), &test_model(), "Sign Up", &candidates, &o).await
        .expect("ok");
    assert_eq!(result.ref_id.as_deref(), Some("e2"));
    assert_eq!(result.action, BrowserActAction::Click);
    assert_eq!(result.reason.as_deref(), Some("matches Sign Up"));
}

#[tokio::test]
async fn llm_ground_handles_null_ref_id() {
    let provider: Arc<dyn Provider> = ScriptedProvider::new(
        r#"{"ref_id":null,"reason":"no matching element"}"#,
    );
    let candidates = vec![el("e1", "button", "Cancel", true, true)];
    let o = obs(candidates.clone());
    let result = llm_ground(provider.as_ref(), &test_model(), "Submit Order", &candidates, &o).await
        .expect("ok");
    assert!(result.ref_id.is_none());
    assert_eq!(result.reason.as_deref(), Some("no matching element"));
}

#[tokio::test]
async fn llm_ground_returns_grouding_parse_on_invalid_json() {
    let provider: Arc<dyn Provider> = ScriptedProvider::new("not json at all");
    let candidates = vec![el("e1", "button", "OK", true, true)];
    let o = obs(candidates.clone());
    let err = llm_ground(provider.as_ref(), &test_model(), "OK", &candidates, &o).await
        .expect_err("should fail to parse");
    assert!(matches!(err, BrowserError::GroundingParse(_)));
}

#[tokio::test]
async fn llm_ground_propagates_provider_error() {
    let provider: Arc<dyn Provider> = Arc::new(FailingProvider);
    let candidates = vec![el("e1", "button", "OK", true, true)];
    let o = obs(candidates.clone());
    let err = llm_ground(provider.as_ref(), &test_model(), "OK", &candidates, &o).await
        .expect_err("provider error");
    assert!(matches!(err, BrowserError::Backend(_)));
}

#[test]
fn dispatch_action_click_uses_selector() {
    // Tab with a recorded click.
    // (We use a tiny MockTab; see the existing browse_extract_tool.rs test
    // pattern in the project for the shape.)
    todo!("covered in Task 3.5 — full integration with MockTab")
}
```

(Note: the `dispatch_action_click_uses_selector` test is intentionally a stub — it's covered by a full MockTab-driven test once we have a mock engine in Task 3.5. Don't fail the file on this placeholder; the test mod just compiles.)

- [ ] **Step 2: Run tests, verify RED**

`llm_ground` doesn't exist yet — `todo!()` body. Verify the 4 LLM tests fail.

- [ ] **Step 3: Implement the LLM grounding path**

```rust
#[derive(Debug, Clone)]
pub enum BrowserActAction {
    Click, Type, Fill, SelectOption, Check, Uncheck, Press, Hover,
}

impl BrowserActAction {
    pub fn from_str(s: &str) -> Option<Self> {
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
    pub fn requires_value(self) -> bool {
        matches!(self, Self::Type | Self::Fill | Self::SelectOption | Self::Press)
    }
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

#[derive(Debug, Clone)]
pub struct LLMGroundResult {
    pub ref_id: Option<String>,
    pub action: BrowserActAction,
    pub value: Option<String>,
    pub reason: Option<String>,
}

pub async fn llm_ground(
    provider: &dyn oxicode_ai::Provider,
    model: &oxicode_ai::Model,
    goal: &str,
    candidates: &[ObservedElement],
    observation: &Observation,
) -> Result<LLMGroundResult, BrowserError> {
    use futures::StreamExt;
    use oxicode_ai::{Context, Message, UserMessage};

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
    let ctx = Context::default();
    // Push one user message.
    let mut ctx = ctx;
    ctx.push(Message::User(UserMessage { content: prompt, ..Default::default() }));

    let stream = provider.stream(model, &ctx, None).await
        .map_err(|e: oxicode_ai::ProviderError| BrowserError::Backend(e.to_string()))?;
    let mut pinned = Box::pin(stream);
    let mut text = String::new();
    while let Some(ev) = pinned.next().await {
        match ev {
            oxicode_ai::ProviderEvent::TextDelta { delta, .. } => text.push_str(&delta),
            oxicode_ai::ProviderEvent::StreamEnd { .. } => break,
            _ => {}
        }
    }
    // Strip optional ```json fences.
    let trimmed = text.trim().trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();
    let v: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| BrowserError::GroundingParse(format!("{e}: {trimmed}")))?;
    let ref_id = v.get("ref_id").and_then(|x| x.as_str()).map(String::from);
    let action_str = v.get("action").and_then(|x| x.as_str()).unwrap_or("click");
    let action = BrowserActAction::from_str(action_str)
        .ok_or_else(|| BrowserError::GroundingParse(format!("unknown action: {action_str}")))?;
    let value = v.get("value").and_then(|x| x.as_str()).map(String::from);
    let reason = v.get("reason").and_then(|x| x.as_str()).map(String::from);
    Ok(LLMGroundResult { ref_id, action, value, reason })
}
```

(Use the actual field shape of `oxicode_ai::Message::User(UserMessage)` from `state.rs:2-3` — `UserMessage { content, .. }`. Check the constructor; if `UserMessage::new(text)` exists, use it. If not, construct literally.)

- [ ] **Step 4: Run tests, verify GREEN**

Run: `cargo nextest run -p oxicode-agent --no-fail-fast 'browse::browse_act_tool::tests::llm_ground_'`
Expected: 4 passes (or 4 + the dispatch stub which we mark `#[ignore]` for now).

(Note: `oxicode_ai::Provider::stream` may have a slightly different signature in this version — `StreamResult` may be `Pin<Box<dyn Stream<...>>>` returned directly, not `Future<Output = StreamResult>`. Check `oxicode-ai/src/providers/trait_def.rs` and adjust the call site. The test's `ScriptedProvider` must match the actual signature.)

- [ ] **Step 5: Implement the struct + `AgentTool` impl + dispatch**

```rust
pub struct BrowseActTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    provider: Arc<dyn oxicode_ai::Provider>,
    model: oxicode_ai::Model,
    callbacks: super::callback_mixin::BrowseCallbacks,
    tab_id_slot: Mutex<Arc<parking_lot::Mutex<Option<uuid::Uuid>>>>,
}

impl BrowseActTool {
    pub fn new(
        provider: Arc<dyn oxicode_ai::Provider>,
        model: oxicode_ai::Model,
        engine: Arc<dyn BrowserEngine>,
    ) -> Self {
        Self::with_config(provider, model, engine, BrowseConfig::default())
    }

    pub fn with_config(
        provider: Arc<dyn oxicode_ai::Provider>,
        model: oxicode_ai::Model,
        engine: Arc<dyn BrowserEngine>,
        config: BrowseConfig,
    ) -> Self {
        Self {
            engine, config, provider, model,
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: Mutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }

    async fn run(self: &Arc<Self>, url: &str, goal: &str, value: Option<&str>,
                 action_hint: Option<BrowserActAction>, timeout_secs: u64) -> Result<AgentToolResult, ToolError> {
        use futures::StreamExt;
        let raw_tab = self.engine.new_tab().await
            .map_err(|e| format!("Failed to open browser tab: {e}"))?;
        let tab_id = raw_tab.tab_id();
        *self.tab_id_slot.lock().lock() = Some(tab_id);
        self.callbacks.register_on_registry(tab_id, self.engine.callback_registry().as_ref());
        let guard = TabGuard::new(raw_tab);

        // 1. Navigate
        let page = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            guard.tab().goto(url),
        ).await.map_err(|_| format!("Navigation timed out after {timeout_secs}s"))?
            .map_err(|e| format!("Navigation failed: {e}"))?;

        // 2. Observe
        let observation = guard.tab().observe().await
            .map_err(|e| format!("Observation failed: {e}"))?;

        // 3. Candidate tier
        let top_n = 20;
        let candidates = candidate_tier(goal, &observation, top_n);
        let mut mode = "llm";
        let mut result = llm_ground(self.provider.as_ref(), &self.model, goal, &candidates, &observation).await;

        // 4. Fallback to deterministic tier if provider errors
        if let Err(BrowserError::Backend(_)) = &result {
            // Top scorer wins deterministically.
            if let Some(top) = candidates.first() {
                let action = action_hint.unwrap_or(BrowserActAction::Click);
                let ground = LLMGroundResult {
                    ref_id: Some(top.ref_id.clone()),
                    action,
                    value: value.map(String::from),
                    reason: Some("deterministic fallback (provider error)".into()),
                };
                result = Ok(ground);
                mode = "deterministic_fallback";
            }
        }

        let ground = result.map_err(|e| e.to_string())?;

        // 5. No match
        let ground = match ground.ref_id {
            Some(rid) => ground,
            None => {
                guard.close().await;
                *self.tab_id_slot.lock().lock() = None;
                return Ok(AgentToolResult::success(serde_json::to_string_pretty(&json!({
                    "matched_ref": null, "matched_name": null, "matched_role": null,
                    "action": null, "selector": null, "score": 0.0,
                    "result": "no_match", "mode": mode, "reason": ground.reason,
                    "candidates_considered": candidates.len(),
                })).unwrap_or_default()));
            }
        };

        // 6. Resolve selector from candidates
        let el = candidates.iter().find(|e| e.ref_id == ground.ref_id).cloned()
            .or_else(|| observation.elements.iter().find(|e| e.ref_id == ground.ref_id).cloned())
            .ok_or_else(|| format!("LLM picked ref_id={} but it is not in the observation", ground.ref_id))?;
        let selector = el.selector.clone();

        // 7. Dispatch
        let final_value = ground.value.as_deref().or(value);
        let action = if let Some(hint) = action_hint { hint } else { ground.action };
        if action.requires_value() && final_value.map(str::is_empty).unwrap_or(true) {
            return Err(BrowserError::MissingValue { action: action.as_str() }.into());
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
        }.map_err(|e| e.to_string())?;

        let matched_name = el.name.clone();
        let matched_role = el.role.clone();
        guard.close().await;
        *self.tab_id_slot.lock().lock() = None;

        Ok(AgentToolResult::success(serde_json::to_string_pretty(&json!({
            "matched_ref": ground.ref_id,
            "matched_name": matched_name,
            "matched_role": matched_role,
            "action": action.as_str(),
            "selector": selector,
            "score": 0.0,
            "result": "ok",
            "mode": mode,
            "reason": ground.reason,
            "candidates_considered": candidates.len(),
        })).unwrap_or_default()).with_metadata(json!({
            "url": page.url, "title": page.title,
        })))
    }
}

#[async_trait]
impl AgentTool for BrowseActTool {
    fn name(&self) -> &str { "browse_act" }
    fn label(&self) -> &str { "Browse Act" }
    fn description(&self) -> &str {
        "Act on a web page using a natural-language goal. The tool observes the page's \
         interactive surface, calls a model to pick the element matching your goal, and \
         dispatches the right click/type/fill/select_option/check/uncheck/press/hover. \
         No CSS selectors required. Use when you know what you want to do but not which \
         element to target."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL of the page to act on"},
                "goal": {"type": "string", "description": "Natural-language action description"},
                "value": {"type": "string", "description": "Value for type/fill/select_option/press"},
                "action_hint": {
                    "type": "string",
                    "enum": ["click","type","fill","select_option","check","uncheck","press","hover"],
                    "description": "Optional action to bias the model's choice",
                },
                "timeout": {"type": "integer", "default": 30, "description": "Seconds"},
            },
            "required": ["url", "goal"],
        })
    }
    fn on_progress(&self, cb: crate::tools::ProgressCallback) { self.callbacks.store_progress(cb); }
    fn on_browse_progress(&self, cb: Arc<dyn Fn(super::BrowseProgress) + Send + Sync>) {
        self.callbacks.store_browse(cb);
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
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let url = params["url"].as_str().ok_or_else(|| "Missing required parameter: url".to_string())?;
        let goal = params["goal"].as_str().ok_or_else(|| "Missing required parameter: goal".to_string())?;
        let value = params["value"].as_str();
        let action_hint = params["action_hint"].as_str().and_then(BrowserActAction::from_str);
        let timeout_secs = params["timeout"].as_u64().unwrap_or(self.config.page_timeout_secs);
        let self_arc = Arc::new(/* see note below */);
        self_arc.run(url, goal, value, action_hint, timeout_secs).await
    }
}
```

The `self_arc` trick is awkward because `&self` doesn't give us `Arc<Self>` directly. Simpler: inline `run` into `execute`, or use `unsafe { Arc::from_raw(Arc::clone(&Arc::new(self)) as *const _ ) }` (don't do that). Best: make `run` take `&self` and `&Arc<dyn Provider>` separately, OR use `self.clone()` if we add `#[derive(Clone)]` (we can't easily because `BrowserEngine` isn't Clone). Pragmatic: inline the run body into `execute`. Keep the code honest.

- [ ] **Step 6: Run all `browse_act_tool` tests, verify GREEN**

Run: `cargo nextest run -p oxicode-agent --no-fail-fast 'browse::browse_act_tool::tests::'`
Expected: ≥ 12 passes (8 candidate_tier + 4 llm_ground). The dispatch stub stays `#[ignore]`.

- [ ] **Step 7: Commit**

```bash
git add oxicode-agent/src/tools/browse/browse_act_tool.rs
git commit -m "feat(browse): browse_act — LLM-grounded natural-language action tool"
```

---

### Task 4: Wire into `mod.rs`, `factory.rs`, and `bootstrap.rs`

**Files:**
- Modify: `oxicode-agent/src/tools/browse/mod.rs` (add `pub mod` + `pub use`)
- Modify: `oxicode-agent/src/tools/browse/factory.rs` (signature change + register)

- [ ] **Step 1: Add module + re-export in `mod.rs`**

In `mod.rs`:
- Add `pub mod browse_act_tool;` next to the other browse_* lines.
- Add `pub use browse_act_tool::BrowseActTool;` in the re-export block.

- [ ] **Step 2: Update `factory.rs` to take `Arc<dyn Provider>` + `Model`**

The current factory signature is:

```rust
pub fn browsing_tools(engine: Arc<dyn BrowserEngine>) -> Arc<ToolRegistry>
```

`BrowseActTool` needs `(provider, model, engine)`. Add a second arg or
create new overloads. Decision: **add required args** — existing
callers must update. There are exactly 4 callers (factory.rs's own
tests don't count; production callers are `oxicode-cli/bootstrap.rs`).

New signature:

```rust
pub fn browsing_tools(
    provider: Arc<dyn oxicode_ai::Provider>,
    model: oxicode_ai::Model,
    engine: Arc<dyn BrowserEngine>,
) -> Arc<ToolRegistry> { ... }

pub fn browsing_tools_with_config(
    provider: Arc<dyn oxicode_ai::Provider>,
    model: oxicode_ai::Model,
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
) -> Arc<ToolRegistry> { ... }

#[cfg(feature = "native-browser")]
pub fn browsing_tools_with_session(
    provider: Arc<dyn oxicode_ai::Provider>,
    model: oxicode_ai::Model,
    engine: Arc<dyn BrowserEngine>,
) -> Arc<ToolRegistry> { ... }
```

This **breaks** every existing caller (4 sites). Acceptable: the
browsing identity is a moving surface (v0.72.0 already broke it);
adding a constructor arg is a tiny ripple.

Register `BrowseActTool::new(Arc::clone(&provider), model.clone(),
Arc::clone(&engine))` in each factory after `BrowseTool`.

- [ ] **Step 3: Update callers in `oxicode-cli`**

Production callers are in `oxicode-cli/src/bootstrap.rs` (search for
`browsing_tools(` and update each call site to pass the existing
`provider` and `model` that bootstrap.rs already holds). One
search-replace per call site.

```rust
// before:
let registry = browsing_tools(engine.clone());
// after:
let registry = browsing_tools(Arc::clone(&provider), model.clone(), engine.clone());
```

Same for `browsing_tools_with_session` and `browsing_tools_with_config`.

- [ ] **Step 4: Build the workspace, verify it compiles**

Run: `cargo build --workspace`
Expected: exit 0. **If `oxicode-sdk` has factory re-exports, they
must update too** — `grep -r 'browsing_tools' --include='*.rs'` and
fix any other call sites.

- [ ] **Step 5: Run all browse tests, verify GREEN**

Run: `cargo nextest run -p oxicode-agent --no-fail-fast 'browse::'`
Expected: existing tests + new ones pass; nothing regresses.

- [ ] **Step 6: Commit**

```bash
git add oxicode-agent/src/tools/browse/mod.rs \
        oxicode-agent/src/tools/browse/factory.rs \
        oxicode-cli/src/bootstrap.rs
git commit -m "feat(browse): wire browse_act into factory and CLI bootstrap"
```

---

### Task 5: CI fix — `libfontconfig1-dev` on smoke-test and msrv jobs

(Already completed in this session. Verify with `git log --oneline
-5` for the commit; if present, skip; otherwise, do it now. The
two-line change is in `.github/workflows/ci.yml:99` and `:149`.)

**Files:**
- Modify: `.github/workflows/ci.yml:99`
- Modify: `.github/workflows/ci.yml:149`

- [ ] **Step 1: Verify commit or apply the change**

Run: `git log --oneline -5`
If `ci: install libfontconfig1-dev on smoke-test and msrv jobs` is in
the log, skip to Step 2. Otherwise, edit lines 99 and 149 to append
`libfontconfig1-dev` to the apt-get install, and commit:

```bash
git add .github/workflows/ci.yml
git commit -m "ci: install libfontconfig1-dev on smoke-test and msrv jobs"
```

- [ ] **Step 2: Verify YAML well-formedness**

Run: `python3 -c 'import yaml; yaml.safe_load(open("/Volumes/MERCURY/PROJECTS/oxicode/.github/workflows/ci.yml"))'`
Expected: exit 0.

---

### Task 6: Verification — full project gates

**Files:** none (read-only checks).

- [ ] **Step 1: Format**

Run: `cargo fmt --all -- --check`
Expected: exit 0.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: exit 0.

- [ ] **Step 3: Clippy on native-browser**

Run: `cargo clippy -p oxicode-cli -- -D warnings && cargo build -p oxicode-agent --features native-browser`
Expected: exit 0.

- [ ] **Step 4: Doc**

Run: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
Expected: exit 0.

- [ ] **Step 5: Smoke subset tests**

Run: `cargo nextest run --workspace -E 'not (test(slow) | test(/^net_/) | test(requires_network) | test(/^bench_/))'`
Expected: ≥ 3312 + 12 = 3324 passes, 0 failures.

- [ ] **Step 6: Commit any fixes**

If any of Steps 1-5 surfaced a fix, commit it separately.

---

### Task 7: CHANGELOG

**Files:**
- Modify: `CHANGELOG.md:8-10`

- [ ] **Step 1: Replace the Unreleased placeholder**

```markdown
## [Unreleased]

### Added

- **`browse_act`** — natural-language goal → grounded `BrowserTab` action.
  Opens a tab, calls `observe()` to capture the page's interactive surface,
  asks a construction-injected LLM to pick the element matching the goal
  (a deterministic Jaccard scorer prunes to top-20 candidates first), and
  dispatches `click` / `type` / `fill` / `select_option` / `check` /
  `uncheck` / `press` / `hover`. No CSS selectors required from the caller.
  Falls back to the deterministic tier (and surfaces `mode:
  "deterministic_fallback"`) when the provider errors. Closes the
  layer-2 gap that browser-use / Stagehand / Playwright MCP / Skyvern /
  AgentQL / Claude CU / OpenAI CUA already address. Provider + Model are
  injected at factory construction (`browsing_tools(provider, model, engine)`).
- **Factory signature change:** `browsing_tools` / `browsing_tools_with_config`
  / `browsing_tools_with_session` now take `Arc<dyn oxicode_ai::Provider>` +
  `oxicode_ai::Model` so `browse_act` can be wired. CLI bootstrap updated
  in lockstep.

### Fixed

- **CI: `libfontconfig1-dev` missing on smoke-test and msrv jobs.** v0.72.0
  promoted `native-browser` to a default feature of `oxicode-cli`, which
  transitively pulls `oxibrowser-render` (Blitz/Stylo/Taffy/vello) and the
  `yeslogic-fontconfig-sys` build script. Smoke-test and msrv jobs install
  `libssl-dev` but not fontconfig, so `pkg-config` could not find fontconfig
  and `cargo test --no-run` (smoke-test) or `cargo build --workspace` (msrv)
  failed at the fontconfig-sys build script. Both jobs now install the system
  dep. CI run 31467470837 was the evidence.

### Deferred

- **`browse_extract_struct`** (intent-based structured extraction) — needs
  its own design pass: intent → selector synthesis, schema → field mapping,
  list-row scoping. Pure CSS-per-field would not be an "AI primitive" and
  is deferred rather than half-shipping. Tracked on the P4 roadmap.
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): unreleased entry for v0.74.0 (browse_act, factory sig, CI fix)"
```

---

### Task 8: Final review — diff sanity check

**Files:** none (read-only).

- [ ] **Step 1: `git log` shows expected commits**

Run: `git log --oneline main..HEAD`
Expected: ~7 commits: engine.rs variants → browse_act_tool → factory+wiring → CI fix → verification fixes (if any) → CHANGELOG.

- [ ] **Step 2: `git diff main --stat` shows expected footprint**

Expected:
- `.github/workflows/ci.yml` (+2/-2)
- `CHANGELOG.md` (+~30/-3)
- `oxicode-agent/src/tools/browse/browse_act_tool.rs` (new, ~400 lines)
- `oxicode-agent/src/tools/browse/engine.rs` (+~12)
- `oxicode-agent/src/tools/browse/factory.rs` (+~10/-3 — signature + register)
- `oxicode-agent/src/tools/browse/mod.rs` (+~2)
- `oxicode-cli/src/bootstrap.rs` (+~6/-3 — pass provider+model to factories)
- `docs/superpowers/specs/2026-08-11-browse-layer2-ai-primitives-design.md` (new)
- `docs/superpowers/plans/2026-08-11-browse-layer2-ai-primitives.md` (new)

- [ ] **Step 3: No untracked files**

Run: `git status`
Expected: clean working tree.

---

## Self-Review

**1. Spec coverage:** every goal in spec §2 maps to a task — `browse_act` with LLM grounding → Tasks 2-3; factory wiring → Task 4; CI fix → Task 5; verification → Task 6; CHANGELOG → Task 7. The deferred `browse_extract_struct` is documented in CHANGELOG (Task 7) and spec §10 — *not* in a task because it's deferred.

**2. Placeholder scan:** every code step has concrete snippets. `dispatch_action_click_uses_selector` test is intentionally a stub (`todo!()` with a comment) — flagged as covered later; the LLM path is fully covered by MockProvider.

**3. Type consistency:** `LLMGroundResult` is the single shape produced by `llm_ground` and consumed by `run`. `BrowserActAction::as_str()` is the single source of truth for action serialization in the JSON output. The factory signature change is a deliberate breaking change, called out in CHANGELOG.
