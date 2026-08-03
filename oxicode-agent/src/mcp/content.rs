//! MCP content transformation.
//!
//! Converts MCP server content types into plain text suitable for
//! inclusion in agent tool results.

use super::types::McpContent;

/// Transform MCP content blocks into a single text string.
///
/// Text content is passed through, images and resources are formatted
/// as descriptive text markers.
pub fn transform_mcp_content(content: &[McpContent]) -> String {
    let mut parts = Vec::new();

    for item in content {
        match item {
            McpContent::Text { text } => {
                parts.push(text.clone());
            }
            McpContent::Image { mime_type, .. } => {
                parts.push(format!(
                    "[Image content: {}]",
                    mime_type.as_deref().unwrap_or("image/*")
                ));
            }
            McpContent::Resource { resource } => {
                let uri = &resource.uri;
                if let Some(text) = &resource.text {
                    parts.push(format!("[Resource: {uri}]\n{text}"));
                } else if resource.blob.is_some() {
                    parts.push(format!("[Resource: {uri}] (binary)"));
                } else {
                    parts.push(format!("[Resource: {uri}] (empty)"));
                }
            }
        }
    }

    if parts.is_empty() {
        "(empty result)".to_string()
    } else {
        parts.join("\n")
    }
}
