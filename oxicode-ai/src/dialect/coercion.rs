//! Tool-argument shape detection for dialect rendering/parsing.
//!
//! Port of omp's `dialect/coercion.ts`. The dialect renderer needs to know which
//! arguments are *string-typed* so it can emit their values verbatim (no JSON
//! quoting), while non-string arguments are JSON-encoded. The parser uses the
//! same shapes to decide whether a raw parameter value is read literally or
//! JSON-decoded.

use crate::tools::Tool;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};

/// Per-tool argument shape derived from its JSON Schema.
#[derive(Debug, Clone, Default)]
pub struct ToolArgShape {
    /// Argument names whose schema is string-only (emit/parse verbatim).
    pub string_args: HashSet<String>,
    /// The `properties` map from the schema (for order/lookup).
    pub properties: HashMap<String, JsonValue>,
    /// Parameter declaration order.
    pub parameter_order: Vec<String>,
}

/// Build argument shapes for a set of tools.
pub fn build_arg_shapes(tools: &[Tool]) -> HashMap<String, ToolArgShape> {
    let mut shapes = HashMap::new();
    for tool in tools {
        let props = resolve_properties(&tool.parameters);
        let mut string_args = HashSet::new();
        let mut parameter_order = Vec::new();
        // JSON objects preserve insertion order via serde_json's Map when the
        // "preserve_order" feature is on; otherwise order is alphabetical. Either
        // is fine for shape detection — only membership matters here.
        if let Some(obj) = props {
            for (key, schema) in obj {
                parameter_order.push(key.clone());
                if is_string_only_schema(&schema) {
                    string_args.insert(key);
                }
            }
        }
        shapes.insert(
            tool.name.clone(),
            ToolArgShape {
                string_args,
                properties: HashMap::new(),
                parameter_order,
            },
        );
    }
    shapes
}

/// Extract the `properties` object from a tool parameter schema, if present.
fn resolve_properties(parameters: &JsonValue) -> Option<serde_json::Map<String, JsonValue>> {
    let obj = parameters.as_object()?;
    obj.get("properties")?.as_object().cloned()
}

/// Whether a schema denotes a string-only value (string, ignoring `null`).
///
/// Mirrors omp's `isStringOnlySchema`: collect all declared types, drop `null`,
/// and require exactly `{"string"}` to remain.
pub fn is_string_only_schema(schema: &JsonValue) -> bool {
    let mut types = collect_schema_types(schema, 0);
    types.remove("null");
    types.len() == 1 && types.contains("string")
}

/// Collect the JSON type names a schema can take (bounded recursion).
fn collect_schema_types(schema: &JsonValue, depth: usize) -> HashSet<String> {
    let mut out = HashSet::new();
    if depth > 8 {
        return out;
    }
    let Some(node) = schema.as_object() else {
        return out;
    };

    match node.get("type") {
        Some(JsonValue::String(t)) => {
            out.insert(t.clone());
        }
        Some(JsonValue::Array(arr)) => {
            for t in arr {
                if let JsonValue::String(s) = t {
                    out.insert(s.clone());
                }
            }
        }
        _ => {}
    }

    // enum without type → infer types from the enum values.
    if !node.contains_key("type") {
        if let Some(JsonValue::Array(en)) = node.get("enum") {
            for v in en {
                out.insert(json_type_of(v).to_string());
            }
        }
        if let Some(c) = node.get("const") {
            out.insert(json_type_of(c).to_string());
        }
    }

    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(JsonValue::Array(branches)) = node.get(key) {
            for sub in branches {
                out.extend(collect_schema_types(sub, depth + 1));
            }
        }
    }

    out
}

/// The JSON type name of a runtime value.
fn json_type_of(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) | JsonValue::Object(_) => "object",
    }
}

/// Decode a raw parameter value: JSON if it parses, else the literal string.
///
/// Mirrors omp's `decodeValue`. Empty/whitespace values decode to themselves.
pub fn decode_value(raw: &str) -> JsonValue {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return JsonValue::String(trimmed.to_string());
    }
    match serde_json::from_str::<JsonValue>(trimmed) {
        Ok(v) => v,
        Err(_) => JsonValue::String(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, params: JsonValue) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            parameters: params,
        }
    }

    #[test]
    fn detects_string_only_args() {
        let t = tool(
            "write",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "lines": {"type": "integer"},
                    "force": {"type": ["boolean", "null"]},
                }
            }),
        );
        let shapes = build_arg_shapes(&[t]);
        let shape = &shapes["write"];
        assert!(shape.string_args.contains("path"));
        assert!(shape.string_args.contains("content"));
        assert!(!shape.string_args.contains("lines"));
        // ["boolean","null"] → drop null → {boolean}, not string-only.
        assert!(!shape.string_args.contains("force"));
    }

    #[test]
    fn nullable_string_is_string_only() {
        assert!(is_string_only_schema(&json!({"type": ["string", "null"]})));
        assert!(!is_string_only_schema(
            &json!({"type": ["string", "number"]})
        ));
    }

    #[test]
    fn enum_infers_types() {
        assert!(is_string_only_schema(&json!({"enum": ["a", "b"]})));
        assert!(!is_string_only_schema(&json!({"enum": [1, 2]})));
    }

    #[test]
    fn anyof_union_collects_types() {
        let schema = json!({"anyOf": [{"type": "string"}, {"type": "null"}]});
        assert!(is_string_only_schema(&schema));
    }

    #[test]
    fn decode_value_prefers_json() {
        assert_eq!(decode_value("42"), json!(42));
        assert_eq!(decode_value("true"), json!(true));
        assert_eq!(decode_value(r#""hi""#), json!("hi"));
        // Non-JSON falls back to the literal string.
        assert_eq!(decode_value("hello world"), json!("hello world"));
        assert_eq!(decode_value("  "), json!(""));
    }
}
