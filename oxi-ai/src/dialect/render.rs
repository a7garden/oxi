//! Dialect prompt rendering — tool catalog + format-guide injection.
//!
//! Port of omp's `dialect/catalog.ts` + `prompt-template.md`.

use crate::tools::Tool;
use serde_json::json;

/// The prompt template wrapping the tool catalog and the dialect guide.
///
/// Mirrors omp's `dialect/prompt-template.md`. The two placeholders are filled
/// by [`render_inband_tool_prompt`].
const PROMPT_TEMPLATE: &str = r#"# Tools

You may call one or more functions to assist with the user query.
Tool calls are emitted as text using the exact syntax below, not as native provider tool messages.

Available functions are listed inside `<tools></tools>` as one JSON object per line:

<tools>
{{TOOLS}}
</tools>

{{DIALECT}}
"#;

const TOOLS_TOKEN: &str = "{{TOOLS}}";
const DIALECT_TOKEN: &str = "{{DIALECT}}";

/// Render the tool catalog — one JSON object per line.
///
/// Each line is `{"type":"function","function":{"name":..,"description":..,"parameters":..}}`,
/// matching omp's `renderToolCatalog`.
pub fn render_tool_catalog(tools: &[Tool]) -> String {
    tools
        .iter()
        .map(|tool| {
            let obj = json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                },
            });
            serde_json::to_string(&obj).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the full in-band tool prompt: catalog + dialect format guide.
pub fn render_inband_tool_prompt(tools: &[Tool], dialect: super::Dialect) -> String {
    let guide = dialect.prompt();
    let guide = guide.trim();
    PROMPT_TEMPLATE
        .replace(TOOLS_TOKEN, &render_tool_catalog(tools))
        .replace(DIALECT_TOKEN, guide)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Dialect;
    use serde_json::json;

    fn echo_tool() -> Tool {
        Tool {
            name: "echo".to_string(),
            description: "Echo a message".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"msg": {"type": "string"}},
                "required": ["msg"],
            }),
        }
    }

    #[test]
    fn catalog_is_one_json_object_per_line() {
        let tools = vec![echo_tool(), echo_tool()];
        let catalog = render_tool_catalog(&tools);
        let lines: Vec<&str> = catalog.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["type"], "function");
            assert_eq!(v["function"]["name"], "echo");
        }
    }

    #[test]
    fn prompt_substitutes_both_tokens() {
        let prompt = render_inband_tool_prompt(&[echo_tool()], Dialect::Xml);
        assert!(!prompt.contains(TOOLS_TOKEN));
        assert!(!prompt.contains(DIALECT_TOKEN));
        assert!(prompt.contains("<tools>"));
        assert!(prompt.contains("\"name\":\"echo\""));
        // The XML format guide is injected.
        assert!(prompt.contains("<invoke"));
    }
}
