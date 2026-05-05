//! Google Generative AI provider (Gemini API)

use async_trait::async_trait;
use futures::stream::StreamExt;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;

use super::google_shared::{
    build_request_body, convert_messages, convert_tools, create_error_message, parse_google_events,
};
use super::shared_client;
use super::{Provider, ProviderError, ProviderEvent, StreamOptions};
use crate::{Api, Context, Model, StopReason};

/// Google Generative AI provider
#[derive(Clone)]
pub struct GoogleProvider {
    client: &'static Client,
    api_key: Option<String>,
}

impl GoogleProvider {
    pub fn new() -> Self {
        Self {
            client: shared_client(),
            api_key: std::env::var("GOOGLE_API_KEY").ok(),
        }
    }

    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        Self {
            client: shared_client(),
            api_key: Some(api_key.into()),
        }
    }
}

impl Default for GoogleProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GoogleProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<StreamOptions>,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let options = options.unwrap_or_default();

        // Get API key
        let api_key = options
            .api_key
            .as_ref()
            .or(self.api_key.as_ref())
            .ok_or_else(|| ProviderError::MissingApiKey)?;

        // Build the request URL (without key - uses header instead for security)
        let model_id = &model.id;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
            model_id
        );

        // Build contents using shared conversion
        let contents = convert_messages(context)?;

        // Build tools using shared conversion
        let tools_json = convert_tools(&context.tools, false);

        // Build request body using shared helper
        let body = build_request_body(
            &contents,
            context.system_prompt.as_deref(),
            tools_json.as_ref(),
            options.temperature,
            options.max_tokens,
        );

        // Make request with API key in header (not URL query param)
        let response = self
            .client
            .post(&url)
            .header("x-goog-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::RequestFailed)?;

        if !response.status().is_success() {
            let status = response.status();
            let body: String = response.text().await.unwrap_or_default();
            return Err(ProviderError::HttpError(status.as_u16(), body));
        }

        // Create event stream
        let model_name = model.id.clone();

        let stream = response.bytes_stream().flat_map(move |chunk| match chunk {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                futures::stream::iter(parse_google_events(
                    &text,
                    Api::GoogleGenerativeAi,
                    "google",
                    &model_name,
                ))
            }
            Err(e) => futures::stream::iter(vec![ProviderEvent::Error {
                reason: StopReason::Error,
                error: create_error_message(
                    Api::GoogleGenerativeAi,
                    "google",
                    &e.to_string(),
                ),
            }]),
        });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "google"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Message};

    #[test]
    fn test_google_provider_name() {
        let provider = GoogleProvider::new();
        assert_eq!(provider.name(), "google");
    }

    #[test]
    fn test_build_google_contents_with_text() {
        let mut ctx = Context::new();
        ctx.add_message(Message::user("Hello, world!"));

        let contents = convert_messages(&ctx).unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello, world!");
    }

    #[test]
    fn test_build_google_tools() {
        let tools = vec![crate::Tool::new(
            "get_weather",
            "Get weather for a location",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "The city name"
                    }
                },
                "required": ["location"]
            }),
        )];

        let tools_json = convert_tools(&tools, false).unwrap();
        let declarations = tools_json[0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0]["name"], "get_weather");
    }

    #[test]
    fn test_parse_google_events_basic_text() {
        let sse_data = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;
        let events = parse_google_events(sse_data, Api::GoogleGenerativeAi, "google", "gemini-1.5-pro");
        assert!(!events.is_empty());
    }

    #[test]
    fn test_create_error_message() {
        let msg = create_error_message(Api::GoogleGenerativeAi, "google", "Something went wrong");
        assert_eq!(msg.provider, "google");
        assert_eq!(msg.api, Api::GoogleGenerativeAi);
        assert_eq!(msg.stop_reason, StopReason::Error);
    }
}
