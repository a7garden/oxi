//! Tool definitions and validation

use jsonschema::Validator;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value as JsonValue;
use std::sync::Arc;
use thiserror::Error;

/// Callback type for progress updates
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Create a progress callback from a closure
pub fn progress_callback<F: Fn(String) + Send + Sync + 'static>(f: F) -> ProgressCallback {
    Arc::new(f)
}

/// Tool definition with JSON Schema parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// Tool name
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// JSON Schema for parameters
    pub parameters: JsonValue,
}

impl Tool {
    /// Create a new tool with the given name, description, and JSON Schema.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxi_ai::Tool;
    /// let tool = Tool::new(
    ///     "read_file",
    ///     "Read contents from a file",
    ///     serde_json::json!({
    ///         "type": "object",
    ///         "properties": {
    ///             "path": {
    ///                 "type": "string",
    ///                 "description": "File path to read"
    ///             }
    ///         },
    ///         "required": ["path"]
    ///     }),
    /// );
    /// ```
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: JsonValue,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Create a simple tool with a single string parameter
    pub fn with_string_param(
        name: impl Into<String>,
        description: impl Into<String>,
        param_name: impl Into<String>,
        param_description: impl Into<String>,
    ) -> Self {
        let param_name = param_name.into();
        let param_description = param_description.into();

        // Build properties manually to avoid borrow issues
        let mut properties = serde_json::Map::new();
        properties.insert("type".to_string(), json!("object"));

        let mut obj_properties = serde_json::Map::new();
        obj_properties.insert(
            param_name.clone(),
            json!({
                "type": "string",
                "description": param_description
            }),
        );
        properties.insert(
            "properties".to_string(),
            serde_json::Value::Object(obj_properties),
        );

        let required_arr =
            serde_json::Value::Array(vec![serde_json::Value::String(param_name.clone())]);
        properties.insert("required".to_string(), required_arr);

        let params = serde_json::Value::Object(properties);
        Self::new(name, description, params)
    }

    /// Validate arguments against the tool's JSON Schema
    pub fn validate(&self, args: &JsonValue) -> Result<JsonValue, ValidationError> {
        validate_args_internal(&self.parameters, args)
    }

    /// Check if this tool requires parameters
    pub fn requires_parameters(&self) -> bool {
        self.parameters
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false)
    }
}

/// Validation error
#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),

    #[error("Schema validation failed: {0}")]
    SchemaValidation(String),

    #[error("Missing required field: {0}")]
    MissingRequiredField(String),
}

/// Validate tool arguments against a JSON Schema
pub fn validate_args(tool: &Tool, args: &JsonValue) -> Result<JsonValue, ValidationError> {
    validate_args_internal(&tool.parameters, args)
}

/// Internal validation implementation
fn validate_args_internal(
    schema: &JsonValue,
    args: &JsonValue,
) -> Result<JsonValue, ValidationError> {
    let validator =
        Validator::new(schema).map_err(|e| ValidationError::SchemaValidation(e.to_string()))?;

    let validation_result = validator.validate(args);

    match validation_result {
        Ok(()) => Ok(args.clone()),
        Err(errors) => {
            // jsonschema returns an error with formatted message
            let error_msg = format!("{}", errors);
            Err(ValidationError::SchemaValidation(error_msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_validation() {
        let tool = Tool::with_string_param(
            "get_weather",
            "Get current weather for a location",
            "location",
            "City name or coordinates",
        );

        let valid_args = serde_json::json!({
            "location": "London"
        });

        let result = tool.validate(&valid_args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_validation_failure() {
        let tool = Tool::with_string_param(
            "get_weather",
            "Get current weather for a location",
            "location",
            "City name or coordinates",
        );

        // Missing required field
        let invalid_args = serde_json::json!({});

        let result = tool.validate(&invalid_args);
        assert!(result.is_err());
    }
}
