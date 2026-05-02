//! Tool definitions and validation

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use jsonschema::Validator;
use thiserror::Error;

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
    /// Create a new tool with the given name, description, and JSON Schema
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: JsonValue) -> Self {
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
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                param_name: {
                    "type": "string",
                    "description": param_description
                }
            },
            "required": [param_name]
        });
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
fn validate_args_internal(schema: &JsonValue, args: &JsonValue) -> Result<JsonValue, ValidationError> {
    let validator = Validator::new(schema)
        .map_err(|e| ValidationError::SchemaValidation(e.to_string()))?;
    
    let validation_result = validator.validate(args);
    
    if let Some(errors) = validation_result {
        let error_messages: Vec<String> = errors
            .map(|e| e.to_string())
            .collect();
        
        if !error_messages.is_empty() {
            return Err(ValidationError::SchemaValidation(error_messages.join("; ")));
        }
    }
    
    Ok(args.clone())
}

/// Create a JSON Schema from a TypeScript-like definition
pub fn create_schema(fields: &[(&str, &str, &str)]) -> JsonValue {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<&str> = Vec::new();
    
    for (name, schema_type, description) in fields {
        let prop = serde_json::json!({
            "type": schema_type,
            "description": description
        });
        properties.insert(name.to_string(), prop);
        required.push(name);
    }
    
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
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
