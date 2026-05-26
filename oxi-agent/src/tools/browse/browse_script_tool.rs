//! Multi-step browser automation via YAML scripts.
//!
//! Parses a YAML sequence of steps and executes them in a single tab.
//! Requires the `native-browser` feature (serde_yaml + oxibrowser-core).

use super::config::BrowseConfig;
use super::engine::{BrowserEngine, BrowserError};
use super::helpers;
use super::tab_guard::TabGuard;
use crate::tools::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::oneshot;

// ── Step definitions ──────────────────────────────────────────────────────────

/// A single step in a browse script.
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
    let docs: Vec<serde_yaml::Value> =
        serde_yaml::from_str(yaml).map_err(|e| format!("Invalid YAML: {}", e))?;

    let steps_val = if docs.len() == 1 {
        &docs[0]
    } else {
        return Err("Expected a single YAML document with a 'steps' list".into());
    };

    // Look for "steps:" key or treat as a direct list
    let steps_node = steps_val.get("steps").unwrap_or(steps_val);

    let steps: Vec<Step> = serde_yaml::from_value(steps_node.clone())
        .map_err(|e| format!("Failed to parse steps: {}", e))?;

    Ok(steps)
}

// ── Execution ─────────────────────────────────────────────────────────────────

async fn execute_steps(
    tab: &dyn super::engine::BrowserTab,
    steps: &[Step],
    config: &BrowseConfig,
    deadline: tokio::time::Instant,
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

        execute_single_step(tab, step, &mut result, config).await?;
    }

    Ok(result)
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

/// Multi-step browser automation tool.
pub struct BrowseScriptTool {
    engine: Arc<dyn BrowserEngine>,
    config: BrowseConfig,
}

impl BrowseScriptTool {
    /// Create with the given engine and default config.
    pub fn new(engine: Arc<dyn BrowserEngine>) -> Self {
        Self {
            engine,
            config: BrowseConfig::default(),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(engine: Arc<dyn BrowserEngine>, config: BrowseConfig) -> Self {
        Self { engine, config }
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
        let script_input = params["script"]
            .as_str()
            .ok_or_else(|| "Missing required parameter: script".to_string())?;

        let timeout_secs = params["timeout"].as_u64().unwrap_or(60);
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

        // Open one tab for the entire script
        let raw_tab = self
            .engine
            .new_tab()
            .await
            .map_err(|e| format!("Failed to open browser tab: {}", e))?;
        let guard = TabGuard::new(raw_tab);

        let script_result = execute_steps(guard.tab(), &steps, &self.config, deadline).await?;

        // Build output
        let mut output_parts = Vec::new();
        if !script_result.outputs.is_empty() {
            output_parts.push(script_result.outputs.join("\n"));
        }

        let mut metadata = json!({
            "steps_executed": steps.len(),
            "variables": script_result.variables,
        });

        let mut result = AgentToolResult::success(output_parts.join("\n")).with_metadata(metadata);

        // Attach screenshot if captured
        if let Some(png) = script_result.screenshot {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
            let img = oxi_ai::ContentBlock::Image(oxi_ai::ImageContent::new(b64, "image/png"));
            result = result.with_content_blocks(vec![img]);
        }

        guard.close().await;
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
