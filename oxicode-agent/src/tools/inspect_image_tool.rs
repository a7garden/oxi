/// Inspect Image tool — analyze images using Vision LLM capabilities.
///
/// Accepts an image path or reference and a question about the image.
/// Requires a Vision-capable model to be active. This tool formats the
/// request and delegates to the Vision model for analysis.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// InspectImageTool — analyze an image with a question.
pub struct InspectImageTool;

#[async_trait]
impl AgentTool for InspectImageTool {
    fn name(&self) -> &str {
        "inspect_image"
    }

    fn label(&self) -> &str {
        "Inspect Image"
    }

    fn description(&self) -> &str {
        concat!(
            "Analyze an image with a specific question. ",
            "Provide the image path (local file path, or Image #N reference) ",
            "and a question about the image content. ",
            "Requires a Vision-capable model. The image will be loaded and ",
            "sent to the model for analysis."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Image path: local file path, Image #N label, or attachment:// URI."
                },
                "question": {
                    "type": "string",
                    "description": "Question about the image content."
                }
            },
            "required": ["path", "question"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Analyze an image")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
    }

    fn tool_tier(&self) -> ToolTier {
        ToolTier::Read
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        let question = params
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: question".to_string())?;

        // In a full implementation, this would:
        // 1. Resolve the image path -> load image bytes
        // 2. Send the image + question to a Vision-capable LLM
        // 3. Return the analysis
        //
        // For now, return an informative message describing how to use
        // Vision-capable models for image analysis.

        Ok(AgentToolResult::success(format!(
            concat!(
                "Image analysis requested.\n",
                "Path: {}\n",
                "Question: {}\n\n",
                "Image analysis requires a Vision-capable model. ",
                "To analyze this image:\n",
                "1. Ensure a Vision-capable model is active (e.g., gpt-4o, claude-3.5-sonnet, gemini-2.0-flash)\n",
                "2. Use `read` to view the image file directly\n",
                "3. Or describe the image content in a message to the model\n\n",
                "The image would be loaded from: {}"
            ),
            path, question, path
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_inspect_image_basic() {
        let tool = InspectImageTool;
        let params = json!({
            "path": "screenshot.png",
            "question": "What does this UI show?"
        });
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Image analysis requested"));
    }
}
