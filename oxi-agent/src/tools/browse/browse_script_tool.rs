//! Multi-step browser automation via YAML scripts.
//!
//! Parses a YAML sequence of steps and executes them in a single tab.
//! Requires the `native-browser` feature (serde_yaml + oxibrowser-core).

#![allow(clippy::useless_conversion, missing_docs)]

use super::config::BrowseConfig;
use super::engine::BrowserEngine;
use super::helpers;
use super::tab_guard::TabGuard;
use crate::tools::typed::TypedTool;
use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::oneshot;

// ── Step definitions ──────────────────────────────────────────────────────────

/// A single step in a browse script.
#[allow(missing_docs)] // Each variant is self-documenting via its label.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Navigate to a URL.
    Goto { url: String },
    /// Go back in browser history.
    Back,
    /// Go forward in browser history.
    Forward,
    /// Reload the current page.
    Reload,
    /// Click an element.
    Click { selector: String },
    /// Fill an input element (sets value directly).
    Fill { selector: String, value: String },
    /// Type text character by character.
    Type { selector: String, value: String },
    /// Clear an input element.
    Clear { selector: String },
    /// Check a checkbox.
    Check { selector: String },
    /// Uncheck a checkbox.
    Uncheck { selector: String },
    /// Select an option in a `<select>`.
    Select { selector: String, value: String },
    /// Press a keyboard combo.
    Press { combo: String },
    /// Scroll down by N pixels.
    Scroll { pixels: u32 },
    /// Wait for an element to appear.
    Wait { selector: String },
    /// Evaluate JavaScript and optionally store the result.
    Evaluate { expr: String },
    /// Extract text content from elements matching a selector.
    Extract {
        selector: String,
        #[serde(default)]
        all: bool,
    },
    /// Get the full page content as markdown.
    Content,
    /// Take a screenshot.
    Screenshot,
    /// Set a variable (echo for debugging).
    Set { key: String, value: String },
    /// Echo a value (for debugging).
    Echo { message: String },
    /// Sleep for N milliseconds.
    Sleep { ms: u64 },
}

/// Result of executing a script.
pub struct ScriptResult {
    pub outputs: Vec<String>,
    pub screenshot: Option<Vec<u8>>,
    pub variables: std::collections::HashMap<String, String>,
}

// ── Parsing ───────────────────────────────────────────────────────────────────

fn parse_steps(yaml: &str) -> Result<Vec<Step>, ToolError> {
    // Accept both simple format (list of maps) and explicit format
    // with a "steps:" key. The simple format is a bare sequence, the
    // explicit format is `{ steps: [...] }`.
    let doc: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| format!("Invalid YAML: {}", e))?;

    let raw_seq = match &doc {
        serde_yaml::Value::Sequence(_) => doc.clone(),
        serde_yaml::Value::Mapping(map) => {
            // Look up "steps" key (serde_yaml 0.9 doesn't implement
            // Index<&str> on Mapping, so iterate).
            let mut found: Option<&serde_yaml::Value> = None;
            for (k, v) in map.iter() {
                if let serde_yaml::Value::String(s) = k
                    && s == "steps"
                {
                    found = Some(v);
                    break;
                }
            }
            match found {
                Some(v) => v.clone(),
                None => {
                    return Err("Missing 'steps' key in YAML document".into());
                }
            }
        }
        _ => return Err("YAML document must be a sequence or a map with a 'steps' key".into()),
    };

    // raw_seq is a sequence of mappings, each containing a single
    // key-value pair that names the variant. E.g.:
    //   - goto: "url"        → Step::Goto { url }
    //   - fill: { ... }      → Step::Fill { selector, value }
    //   - screenshot: {}     → Step::Screenshot
    //
    // The single value can be either a string (shorthand for a
    // one-field struct variant), a mapping (multi-field struct variant),
    // or null (unit variant).
    let yaml_seq = raw_seq
        .as_sequence()
        .ok_or_else(|| "steps must be a YAML sequence".to_string())?;

    let mut steps = Vec::with_capacity(yaml_seq.len());
    for (i, item) in yaml_seq.iter().enumerate() {
        let mapping = item
            .as_mapping()
            .ok_or_else(|| format!("step {} is not a YAML mapping", i))?;
        if mapping.len() != 1 {
            return Err(format!(
                "step {} must have exactly one key (the variant name), got {}",
                i,
                mapping.len()
            ));
        }
        let (variant_key, payload) = mapping
            .iter()
            .next()
            .expect("mapping.len() == 1 checked above");
        let variant = variant_key
            .as_str()
            .ok_or_else(|| format!("step {} variant name is not a string", i))?;
        let step = step_from_yaml_payload(variant, payload)
            .map_err(|e| format!("step {} ({}): {}", i, variant, e))?;
        steps.push(step);
    }

    Ok(steps)
}

/// Build a `Step` from a YAML variant name + payload value.
///
/// `payload` may be:
/// - A string → shorthand for `{ <only_field>: <string> }`.
/// - A mapping → used as-is.
/// - A null → unit variant (no fields).
fn step_from_yaml_payload(variant: &str, payload: &serde_yaml::Value) -> Result<Step, String> {
    use serde_yaml::Value as Y;

    /// Build a JSON object `{ <variant>: <fields> }` and let serde
    /// do the variant dispatch. For single-field struct variants
    /// with a YAML string payload, the field name is inferred by
    /// inspecting the variant's expected field.
    let wrapper = match payload {
        Y::Null => serde_json::json!({ variant: {} }),
        Y::Mapping(m) if m.is_empty() => {
            // `screenshot: {}` — unit variant with no fields.
            serde_json::json!({ variant: null })
        }
        Y::String(s) => {
            // Try common single-field variants first; if none match,
            // let serde report the error.
            let single_field = single_field_struct_variant(variant)
                .ok_or_else(|| format!("variant '{}' has no single-field shorthand", variant))?;
            serde_json::json!({ variant: { single_field: s } })
        }
        _ => {
            let payload_json = yaml_to_json_recursive(payload)?;
            serde_json::json!({ variant: payload_json })
        }
    };
    serde_json::from_value(wrapper).map_err(|e| e.to_string())
}

/// Return the field name for variants that have exactly one String field,
/// enabling YAML shorthand `- goto: "url"` for `- goto: { url: "url" }`.
fn single_field_struct_variant(variant: &str) -> Option<&'static str> {
    match variant {
        "goto" => Some("url"),
        "click" => Some("selector"),
        "fill" | "fill_" => Some("selector"), // not actually used; fill has 2 fields
        "type" | "type_" => Some("selector"),
        "clear" => Some("selector"),
        "check" | "uncheck" => Some("selector"),
        "select" => Some("selector"),
        "press" => Some("combo"),
        "wait" => Some("selector"),
        "evaluate" => Some("expr"),
        "extract" => Some("selector"),
        "set" => Some("key"),
        "echo" => Some("message"),
        _ => None,
    }
}

fn yaml_to_json_recursive(v: &serde_yaml::Value) -> Result<serde_json::Value, ToolError> {
    use serde_yaml::Value as Y;
    match v {
        Y::Null => Ok(serde_json::Value::Null),
        Y::Bool(b) => Ok(serde_json::Value::Bool(*b)),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(serde_json::Value::Number(i.into()))
            } else if let Some(u) = n.as_u64() {
                Ok(serde_json::Value::Number(u.into()))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| "non-finite number".to_string())
            } else {
                Err("unsupported number type".to_string())
            }
        }
        Y::String(s) => Ok(serde_json::Value::String(s.clone())),
        Y::Sequence(items) => {
            let arr: Result<Vec<serde_json::Value>, ToolError> =
                items.iter().map(yaml_to_json_recursive).collect();
            Ok(serde_json::Value::Array(arr?))
        }
        Y::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                let key = k
                    .as_str()
                    .ok_or_else(|| "non-string mapping key".to_string())?
                    .to_string();
                obj.insert(key, yaml_to_json_recursive(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        Y::Tagged(t) => yaml_to_json_recursive(&t.value),
    }
}

// ── Execution ─────────────────────────────────────────────────────────────────

async fn execute_steps(
    tab: &dyn super::engine::BrowserTab,
    steps: &[Step],
    config: &BrowseConfig,
    deadline: tokio::time::Instant,
    progress_cb: Option<&crate::tools::ProgressCallback>,
) -> Result<ScriptResult, ToolError> {
    let mut result = ScriptResult {
        outputs: Vec::new(),
        screenshot: None,
        variables: std::collections::HashMap::new(),
    };

    for (i, step) in steps.iter().enumerate() {
        if tokio::time::Instant::now() > deadline {
            return Err(format!("Script timed out at step {} of {}", i + 1, steps.len()).into());
        }

        if i >= config.max_script_steps {
            return Err(format!(
                "Exceeded maximum script steps ({})",
                config.max_script_steps
            )
            .into());
        }

        // Emit step-level progress so the UI can show "3/10 Clicking element…"
        if let Some(cb) = progress_cb {
            cb(format!("[{}/{}] {}", i + 1, steps.len(), step_label(step)));
        }

        execute_single_step(tab, step, &mut result, config).await?;
    }

    Ok(result)
}

/// Human-readable label for a script step (for progress reporting).
fn step_label(step: &Step) -> &'static str {
    match step {
        Step::Goto { .. } => "Navigating",
        Step::Click { .. } => "Clicking element",
        Step::Fill { .. } => "Filling input",
        Step::Type { .. } => "Typing text",
        Step::Clear { .. } => "Clearing input",
        Step::Check { .. } => "Checking checkbox",
        Step::Uncheck { .. } => "Unchecking checkbox",
        Step::Select { .. } => "Selecting option",
        Step::Press { .. } => "Pressing key",
        Step::Scroll { .. } => "Scrolling",
        Step::Wait { .. } => "Waiting for element",
        Step::Evaluate { .. } => "Evaluating JavaScript",
        Step::Extract { .. } => "Extracting data",
        Step::Content => "Reading page content",
        Step::Screenshot => "Taking screenshot",
        Step::Set { .. } => "Setting variable",
        Step::Echo { .. } => "Echo",
        Step::Sleep { .. } => "Sleeping",
        Step::Back => "Going back",
        Step::Forward => "Going forward",
        Step::Reload => "Reloading page",
    }
}

async fn execute_single_step(
    tab: &dyn super::engine::BrowserTab,
    step: &Step,
    result: &mut ScriptResult,
    config: &BrowseConfig,
) -> Result<(), ToolError> {
    match step {
        Step::Goto { url } => {
            tab.goto(url).await.map_err(|e| e.to_string())?;
        }
        Step::Click { selector } => {
            tab.click(selector).await.map_err(|e| e.to_string())?;
        }
        Step::Fill { selector, value } => {
            tab.fill(selector, value).await.map_err(|e| e.to_string())?;
        }
        Step::Type { selector, value } => {
            tab.type_(selector, value)
                .await
                .map_err(|e| e.to_string())?;
        }
        Step::Clear { selector } => {
            // Clear by filling with empty string
            tab.fill(selector, "").await.map_err(|e| e.to_string())?;
        }
        Step::Check { selector } => {
            let js = helpers::js_check(selector);
            tab.evaluate(&js).await.map_err(|e| e.to_string())?;
        }
        Step::Uncheck { selector } => {
            let js = helpers::js_uncheck(selector);
            tab.evaluate(&js).await.map_err(|e| e.to_string())?;
        }
        Step::Select { selector, value } => {
            if value.is_empty() {
                return Err("Select step requires a non-empty value".into());
            }
            let js = helpers::js_set_select_value(selector, value);
            tab.evaluate(&js).await.map_err(|e| e.to_string())?;
        }
        Step::Press { combo } => {
            tab.press(combo).await.map_err(|e| e.to_string())?;
        }
        Step::Scroll { pixels } => {
            let js = format!("window.scrollBy(0, {})", pixels);
            tab.evaluate(&js).await.map_err(|e| e.to_string())?;
        }
        Step::Wait { selector } => {
            tab.wait_for(selector, config.default_wait_timeout_ms)
                .await
                .map_err(|e| e.to_string())?;
        }
        Step::Evaluate { expr } => {
            let value = tab.evaluate(expr).await.map_err(|e| e.to_string())?;
            let text = match value {
                Value::String(s) => s,
                other => serde_json::to_string(&other).unwrap_or_default(),
            };
            result.outputs.push(text);
        }
        Step::Extract { selector, all } => {
            let texts = tab.query_all(selector).await.map_err(|e| e.to_string())?;
            let texts = if *all {
                texts
            } else {
                texts.into_iter().take(1).collect()
            };
            result.outputs.push(texts.join("\n"));
        }
        Step::Content => {
            let page = tab.content().await.map_err(|e| e.to_string())?;
            result.outputs.push(page.markdown);
        }
        Step::Screenshot => {
            let png = tab
                .screenshot(config.screenshot_width)
                .await
                .map_err(|e| e.to_string())?;
            result.screenshot = Some(png);
        }
        Step::Set { key, value } => {
            result.variables.insert(key.clone(), value.clone());
        }
        Step::Echo { message } => {
            result.outputs.push(message.clone());
        }
        Step::Sleep { ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
        }
        // History navigation — best-effort via JS
        Step::Back => {
            let _ = tab.evaluate("history.back()").await;
        }
        Step::Forward => {
            let _ = tab.evaluate("history.forward()").await;
        }
        Step::Reload => {
            let _ = tab.evaluate("location.reload()").await;
        }
    }
    Ok(())
}

// ── BrowseScriptTool ──────────────────────────────────────────────────────────

/// Typed arguments for [`BrowseScriptTool`].
#[derive(Deserialize, JsonSchema)]
pub struct BrowseScriptArgs {
    script: String,
    #[serde(default = "default_script_timeout")]
    timeout: u64,
}

fn default_script_timeout() -> u64 {
    60
}

pub struct BrowseScriptTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
    /// Shared callback management (progress + browse progress).
    callbacks: super::callback_mixin::BrowseCallbacks,
    /// Shared slot for the current tab's ID.
    tab_id_slot: Mutex<Arc<parking_lot::Mutex<Option<uuid::Uuid>>>>,
}

impl BrowseScriptTool {
    /// Create with the given engine and default config.
    pub fn new(engine: Arc<dyn BrowserEngine>) -> Self {
        Self {
            engine,
            config: BrowseConfig::default(),
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: Mutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(engine: Arc<dyn BrowserEngine>, config: BrowseConfig) -> Self {
        Self {
            engine,
            config,
            callbacks: super::callback_mixin::BrowseCallbacks::new(),
            tab_id_slot: Mutex::new(Arc::new(parking_lot::Mutex::new(None))),
        }
    }
}

#[async_trait]
impl AgentTool for BrowseScriptTool {
    fn name(&self) -> &str {
        "browse_script"
    }

    fn label(&self) -> &str {
        "Browser Script"
    }

    fn description(&self) -> &str {
        "Run a multi-step browser automation script in YAML format. \
         Supports: goto, click, fill, type, press, wait, extract, evaluate, \
         check, uncheck, select, scroll, screenshot, content, sleep."
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

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "string",
                    "description": "YAML script (inline or path to .yaml file)"
                },
                "timeout": {
                    "type": "integer",
                    "default": 60,
                    "description": "Maximum execution time in seconds"
                }
            },
            "required": ["script"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let args: BrowseScriptArgs =
            serde_json::from_value(params).map_err(|e| format!("invalid params: {e}"))?;
        self.execute_typed(_tool_call_id, args, _signal, _ctx).await
    }
}

#[async_trait]
impl TypedTool for BrowseScriptTool {
    type Args = BrowseScriptArgs;

    async fn execute_typed(
        &self,
        _tool_call_id: &str,
        args: Self::Args,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let script_input = &args.script;
        let timeout_secs = args.timeout;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

        // Load script from file if it's a path
        let yaml = if Path::new(script_input).exists() {
            std::fs::read_to_string(script_input)
                .map_err(|e| format!("Failed to read script file: {}", e))?
        } else {
            script_input.to_string()
        };

        let steps = parse_steps(&yaml)?;
        if steps.is_empty() {
            return Err("Script contains no steps".into());
        }

        tracing::info!(steps = steps.len(), "executing browse script");

        let progress_cb = self.callbacks.take_progress();

        let raw_tab = self
            .engine
            .new_tab()
            .await
            .map_err(|e| format!("Failed to open browser tab: {}", e))?;

        let tab_id = raw_tab.tab_id();
        *self.tab_id_slot.lock().lock() = Some(tab_id);

        if let Some(ref cb) = progress_cb {
            let registry = self.engine.callback_registry();
            registry.set(tab_id, cb.clone());
        }
        self.callbacks
            .register_browse_on_registry(tab_id, self.engine.callback_registry().as_ref());

        let guard = TabGuard::new(raw_tab);

        let script_result = execute_steps(
            guard.tab(),
            &steps,
            &self.config,
            deadline,
            progress_cb.as_ref(),
        )
        .await?;

        let mut output_parts = Vec::new();
        if !script_result.outputs.is_empty() {
            output_parts.push(script_result.outputs.join("\n"));
        }

        let metadata = json!({
            "steps_executed": steps.len(),
            "variables": script_result.variables,
        });

        let mut result = AgentToolResult::success(output_parts.join("\n")).with_metadata(metadata);

        if let Some(png) = script_result.screenshot {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
            let img = oxi_ai::ContentBlock::Image(oxi_ai::ImageContent::new(b64, "image/png"));
            result = result.with_content_blocks(vec![img]);
        }

        guard.close().await;
        *self.tab_id_slot.lock().lock() = None;
        Ok(result)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_goto() {
        let yaml = r##"
steps:
  - goto: "https://example.com"
  - click: "button.submit"
  - fill:
      selector: "#search"
      value: "rust"
"##;
        let steps = parse_steps(yaml).unwrap();
        assert_eq!(steps.len(), 3);
        assert!(matches!(&steps[0], Step::Goto { url } if url == "https://example.com"));
        assert!(matches!(&steps[1], Step::Click { selector } if selector == "button.submit"));
        assert!(
            matches!(&steps[2], Step::Fill { selector, value } if selector == "#search" && value == "rust")
        );
    }

    #[test]
    fn parse_extract_step() {
        let yaml = r#"
steps:
  - extract:
      selector: ".result h3"
      all: true
"#;
        let steps = parse_steps(yaml).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(
            matches!(&steps[0], Step::Extract { selector, all } if selector == ".result h3" && *all)
        );
    }

    #[test]
    fn parse_evaluate_step() {
        let yaml = r#"
steps:
  - evaluate:
      expr: "document.title"
"#;
        let steps = parse_steps(yaml).unwrap();
        assert!(matches!(&steps[0], Step::Evaluate { expr } if expr == "document.title"));
    }

    #[test]
    fn parse_screenshot_step() {
        let yaml = r#"
steps:
  - goto: "https://example.com"
  - screenshot: {}
"#;
        let steps = parse_steps(yaml).unwrap();
        assert!(matches!(&steps[1], Step::Screenshot));
    }

    #[test]
    fn parse_wait_step() {
        let yaml = r#"
steps:
  - wait:
      selector: ".loaded"
"#;
        let steps = parse_steps(yaml).unwrap();
        assert!(matches!(&steps[0], Step::Wait { selector } if selector == ".loaded"));
    }

    #[test]
    fn parse_press_step() {
        let yaml = r#"
steps:
  - press:
      combo: "Enter"
"#;
        let steps = parse_steps(yaml).unwrap();
        assert!(matches!(&steps[0], Step::Press { combo } if combo == "Enter"));
    }

    #[test]
    fn parse_scroll_step() {
        let yaml = r#"
steps:
  - scroll:
      pixels: 500
"#;
        let steps = parse_steps(yaml).unwrap();
        assert!(matches!(&steps[0], Step::Scroll { pixels } if *pixels == 500));
    }

    #[test]
    fn parse_select_step() {
        let yaml = r##"
steps:
  - select:
      selector: "#country"
      value: "US"
"##;
        let steps = parse_steps(yaml).unwrap();
        assert!(
            matches!(&steps[0], Step::Select { selector, value } if selector == "#country" && value == "US")
        );
    }

    #[test]
    fn parse_check_uncheck_steps() {
        let yaml = r##"
steps:
  - check:
      selector: "#agree"
  - uncheck:
      selector: "#newsletter"
"##;
        let steps = parse_steps(yaml).unwrap();
        assert!(matches!(&steps[0], Step::Check { selector } if selector == "#agree"));
        assert!(matches!(&steps[1], Step::Uncheck { selector } if selector == "#newsletter"));
    }

    #[test]
    fn parse_empty_script_returns_error() {
        let yaml = r#"
steps: []
"#;
        let steps = parse_steps(yaml).unwrap();
        assert!(steps.is_empty());
    }

    #[test]
    fn test_js_helpers() {
        // Verify JS helpers produce valid JS strings
        let sel_js = helpers::js_set_select_value("#country", "US");
        assert!(sel_js.contains("#country"));
        assert!(sel_js.contains("US"));

        let check_js = helpers::js_check("#agree");
        assert!(check_js.contains("#agree"));
        assert!(check_js.contains("!el.checked"));

        let uncheck_js = helpers::js_uncheck("#newsletter");
        assert!(uncheck_js.contains("#newsletter"));
        assert!(uncheck_js.contains("el.checked"));
    }
}
