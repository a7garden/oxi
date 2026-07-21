//! Image generation tool using OpenRouter API.
//!
//! Provides an `AgentTool` that calls OpenRouter's image generation endpoint.
//! Supports models like `black-forest-labs/flux-1-dev`, `openai/dall-e-3`, etc.

use super::http_client::shared_http_client;
use super::{AgentTool, AgentToolResult, ToolContext, ToolError};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose};
use oxi_ai::types::{ImageGenerationRequest, ImageGenerationResponse};
use serde_json::{Value, json};
use std::env;

/// Default image generation model.
const DEFAULT_MODEL: &str = "black-forest-labs/flux-1-dev";

/// OpenRouter API base URL.
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Maximum prompt length to warn about.
const MAX_PROMPT_LEN: usize = 4000;

/// Image generation tool.
pub struct GenerateImageTool;

impl GenerateImageTool {
    /// Create a new GenerateImageTool.
    pub fn new() -> Self {
        Self
    }

    /// Build the request body for OpenRouter.
    fn build_request_body(req: &ImageGenerationRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": req.model.as_deref().unwrap_or(DEFAULT_MODEL),
            "prompt": req.prompt,
        });

        if let Some(size) = &req.size {
            body["size"] = serde_json::json!(size);
        }
        if let Some(n) = req.n {
            body["n"] = serde_json::json!(n);
        }
        if let Some(ref fmt) = req.response_format {
            body["response_format"] = serde_json::json!(fmt);
        }
        body
    }

    /// Call OpenRouter image generation API.
    async fn call_openrouter(
        &self,
        api_key: &str,
        request: &ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ToolError> {
        let url = format!("{}/images/generations", OPENROUTER_BASE_URL);
        let body = Self::build_request_body(request);

        let client = shared_http_client();
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/oxi")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("OpenRouter request failed: {}", e))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {}", e))?;

        if !status.is_success() {
            // Try to extract error message from API response
            let err_msg = {
                let parsed = serde_json::from_str::<serde_json::Value>(&text).ok();
                match parsed {
                    Some(ref root) => root
                        .get("error")
                        .or_else(|| root.get("message"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| text.clone()),
                    None => text.clone(),
                }
            };
            return Err(format!(
                "OpenRouter API error ({}): {}",
                status,
                err_msg.clone()
            ));
        }

        // Parse OpenRouter image response
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("Invalid JSON response: {}", e))?;

        // OpenRouter wraps the standard response under "data"
        let data = parsed
            .get("data")
            .ok_or_else(|| "Missing 'data' field in response".to_string())?
            .as_array()
            .ok_or_else(|| "'data' is not an array".to_string())?;

        let mut images: Vec<Vec<u8>> = Vec::new();
        let mut revised_prompt: Option<String> = None;

        for item in data {
            // b64_json format
            if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
                let bytes = base64_decode(b64)?;
                images.push(bytes);
            }
            // url format (silently included as-is; return URL as string)
            else if let Some(url_str) = item.get("url").and_then(|v| v.as_str()) {
                // Encode URL as bytes for uniform output
                images.push(url_str.as_bytes().to_vec());
            }

            // Capture revised_prompt if present (DALL-E style)
            if revised_prompt.is_none() {
                revised_prompt = item
                    .get("revised_prompt")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
        }

        Ok(ImageGenerationResponse {
            images,
            revised_prompt,
        })
    }
}

impl Default for GenerateImageTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode base64 without the full base64 crate — plain std.
fn base64_decode(input: &str) -> Result<Vec<u8>, ToolError> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = input.as_bytes();
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0;

    for &byte in input
        .iter()
        .filter(|&&b| b != b'=' && b != b'\n' && b != b'\r')
    {
        let val = CHARS
            .iter()
            .position(|&c| c == byte)
            .ok_or_else(|| format!("Invalid base64 character: {:?}", byte as char))?
            as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[async_trait]
impl AgentTool for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn label(&self) -> &str {
        "Generate Image"
    }

    fn description(&self) -> &str {
        "Generate an image from a text prompt using an AI image generation model via OpenRouter. \
         Takes a `prompt` (required), optional `model` (default: black-forest-labs/flux-1-dev), \
         and optional `size`. Returns base64-encoded image data."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Detailed text description of the desired image"
                },
                "model": {
                    "type": "string",
                    "description": "Model to use (e.g. openai/dall-e-3, black-forest-labs/flux-1-dev, stability-ai/stable-diffusion-3). Default: black-forest-labs/flux-1-dev"
                },
                "size": {
                    "type": "string",
                    "description": "Image size (e.g. 1024x1024, 1024x1792). Provider-dependent."
                },
                "n": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "description": "Number of images to generate (1-10, default 1)"
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: Option<tokio::sync::oneshot::Receiver<()>>,
        _ctx: &ToolContext,
    ) -> Result<AgentToolResult, ToolError> {
        // Parse parameters
        let prompt: String = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: prompt".to_string())?
            .to_string();

        if prompt.is_empty() {
            return Err("Prompt cannot be empty".to_string());
        }

        if prompt.chars().count() > MAX_PROMPT_LEN {
            tracing::warn!(
                "Prompt length {} exceeds recommended max {}",
                prompt.chars().count(),
                MAX_PROMPT_LEN
            );
        }

        let request = ImageGenerationRequest {
            prompt: prompt.clone(),
            model: params
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from),
            size: params
                .get("size")
                .and_then(|v| v.as_str())
                .map(String::from),
            n: params.get("n").and_then(|v| v.as_u64()).map(|v| v as u32),
            response_format: Some("b64_json".to_string()),
        };

        let api_key = env::var("OPENROUTER_API_KEY")
            .or_else(|_| env::var("OPENAI_API_KEY"))
            .map_err(|_| {
                "OPENROUTER_API_KEY (or OPENAI_API_KEY) environment variable is not set. \
                     Please set your API key before using the image generation tool."
            })?;

        let response = self.call_openrouter(&api_key, &request).await?;

        if response.images.is_empty() {
            return Ok(AgentToolResult::success(
                "Image generation completed but returned no images.",
            ));
        }

        // Format response for the agent
        let n_images = response.images.len();
        let mut output = format!("Generated {} image(s).\n\n", n_images);

        if let Some(ref revised) = response.revised_prompt {
            output.push_str(&format!("Revised prompt: {}\n\n", revised));
        }

        for (i, img_data) in response.images.iter().enumerate() {
            let b64 = general_purpose::STANDARD.encode(img_data);
            output.push_str(&format!(
                "Image {} ({} bytes, base64):\n{}\n\n",
                i + 1,
                img_data.len(),
                b64
            ));
        }

        Ok(AgentToolResult::success(output.trim_end()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_decode() {
        // "Hello, World!" base64-encoded
        let encoded = "SGVsbG8sIFdvcmxkIQ==";
        let decoded = base64_decode(encoded).unwrap();
        assert_eq!(decoded, b"Hello, World!");
    }

    #[test]
    fn test_build_request_body() {
        let req = ImageGenerationRequest {
            prompt: "A red cat".to_string(),
            model: Some("flux-dev".to_string()),
            size: Some("1024x1024".to_string()),
            n: Some(2),
            response_format: Some("b64_json".to_string()),
        };

        let body = GenerateImageTool::build_request_body(&req);
        assert_eq!(body["prompt"], "A red cat");
        assert_eq!(body["model"], "flux-dev");
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["n"], 2);
        assert_eq!(body["response_format"], "b64_json");
    }

    #[test]
    fn test_default_model() {
        let req = ImageGenerationRequest::default();
        let body = GenerateImageTool::build_request_body(&req);
        assert_eq!(body["model"], DEFAULT_MODEL);
    }

    #[test]
    fn test_image_generation_response_default() {
        let resp = ImageGenerationResponse::default();
        assert!(resp.images.is_empty());
        assert!(resp.revised_prompt.is_none());
    }
}
