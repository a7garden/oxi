/// Computer tool — computer control using Vision AI capabilities.
///
/// Provides a simplified interface for computer interaction through Vision
/// models: screenshot capture, mouse/keyboard actions. Requires a
/// Vision-capable model and appropriate OS-level access (screen capture,
/// input simulation). The omp implementation uses the full `pi-natives`
/// desktop automation layer; this is a practical stub.
use super::{AgentTool, AgentToolResult, ToolContext, ToolError, ToolExecutionMode, ToolTier};
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// ComputerTool — basic computer control interface.
pub struct ComputerTool;

#[async_trait]
impl AgentTool for ComputerTool {
    fn name(&self) -> &str {
        "computer"
    }

    fn label(&self) -> &str {
        "Computer"
    }

    fn description(&self) -> &str {
        concat!(
            "Interact with the computer using Vision AI: capture screenshots, ",
            "move mouse, click, type text, press keys. ",
            "Actions: screenshot (capture screen), mouse_move (x, y), ",
            "left_click, right_click, type (text), key (key name), ",
            "scroll (x, y). Requires OS-level access permissions."
        )
    }

    fn essential(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["screenshot", "mouse_move", "left_click", "right_click", "double_click", "type", "key", "scroll"],
                    "description": "Computer action to perform."
                },
                "x": {
                    "type": "integer",
                    "description": "X coordinate (for mouse_move, scroll)."
                },
                "y": {
                    "type": "integer",
                    "description": "Y coordinate (for mouse_move, scroll)."
                },
                "text": {
                    "type": "string",
                    "description": "Text to type (for type action)."
                },
                "key": {
                    "type": "string",
                    "description": "Key name to press (for key action: Enter, Tab, Escape, etc.)."
                }
            },
            "required": ["action"]
        })
    }

    fn intent(&self) -> Option<&str> {
        Some("Control the computer via Vision AI")
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::SequentialOnly
    }

    fn tool_tier(&self) -> ToolTier {
        ToolTier::Exec
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?;

        // Full implementation would use OS-level APIs (Core Graphics on macOS,
        // X11/Wayland on Linux, Win32 on Windows).
        // For now, provide actionable guidance.

        let response = match action {
            "screenshot" => concat!(
                "Screenshot capture requested.\n",
                "To capture a screenshot, use:\n",
                "  macOS: `screencapture -x /tmp/screenshot.png`\n",
                "  Linux: `import -window root /tmp/screenshot.png`\n",
                "  Then use `read` to view the image.\n\n",
                "Full vision-integrated screenshot requires native desktop ",
                "access (Core Graphics / X11 / Win32) which is not yet implemented."
            )
            .to_string(),
            "mouse_move" | "left_click" | "right_click" | "double_click" | "scroll" => {
                let x = params.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
                let y = params.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
                format!("Action '{}' at ({}, {}) requested.\n", action, x, y)
                    + "Computer control requires OS-level input simulation not yet implemented.\n"
                    + "Use the `bash` tool with platform-specific commands:\n"
                    + "  macOS: `osascript -e 'tell app \"System Events\" to click at {x, y}'`\n"
                    + "  Linux: `xdotool mousemove x y`"
            }
            "type" => {
                let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                format!("Type action requested: '{}'\n", text)
                    + "Keyboard input simulation requires OS-level APIs.\n"
                    + "Use `bash` with platform-specific tools:\n"
                    + "  macOS: `osascript -e 'tell app \"System Events\" to keystroke \"...\"'`\n"
                    + "  Linux: `xdotool type '...'`"
            }
            "key" => {
                let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
                format!("Key press action requested: '{}'\n", key)
                    + "Key simulation requires OS-level APIs.\n"
                    + "Use `bash` with platform-specific tools:\n"
                    + "  macOS: `osascript -e 'tell app \"System Events\" to key code 36'`\n"
                    + "  Linux: `xdotool key Return`"
            }
            _ => format!("Unknown computer action: {}", action),
        };

        Ok(AgentToolResult::success(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_computer_screenshot() {
        let tool = ComputerTool;
        let params = json!({"action": "screenshot"});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Screenshot capture requested"));
    }

    #[tokio::test]
    async fn test_computer_mouse_move() {
        let tool = ComputerTool;
        let params = json!({"action": "mouse_move", "x": 100, "y": 200});
        let result = tool
            .execute("id", params, None, &ToolContext::default())
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("mouse_move"));
    }
}
