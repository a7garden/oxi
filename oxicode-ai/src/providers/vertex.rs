//! Google Vertex AI provider
//!
//! This provider uses Google Cloud authentication (service account or gcloud CLI)
//! to access Vertex AI models via the Gemini API.

use futures::Stream;
use futures::stream::StreamExt;
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;

use super::google_shared::{
    build_request_body, convert_messages, convert_tools, create_error_message, parse_google_events,
};
use super::shared_client;
use super::sse::split_complete_lines;
use super::{Provider, ProviderError, ProviderEvent, StreamOptions, StreamResult};
use crate::{Api, Context, Model, StopReason};

/// Google Vertex AI provider
///
/// Uses Bearer token authentication via:
/// - Service account JSON file (GOOGLE_APPLICATION_CREDENTIALS)
/// - gcloud CLI access token (from `gcloud auth print-access-token`)
#[derive(Clone)]
pub struct VertexProvider {
    client: &'static Client,
}

impl VertexProvider {
    /// Create a new Vertex provider with default settings.
    pub fn new() -> Self {
        Self {
            client: shared_client(),
        }
    }

    async fn get_access_token(&self) -> Result<String, ProviderError> {
        if let Ok(token) = std::env::var("GOOGLE_ACCESS_TOKEN")
            && !token.is_empty()
        {
            return Ok(token);
        }
        if let Ok(token) = Self::get_gcloud_token().await {
            return Ok(token);
        }
        if let Ok(creds) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            && !creds.is_empty()
        {
            return Self::get_token_from_service_account(&creds).await;
        }
        Err(ProviderError::MissingApiKey)
    }

    async fn get_gcloud_token() -> Result<String, ProviderError> {
        use std::io;
        use tokio::process::Command;
        let output = Command::new("gcloud")
            .args(["auth", "print-access-token"])
            .output()
            .await
            .map_err(ProviderError::IoError)?;
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
        Err(ProviderError::IoError(io::Error::new(
            io::ErrorKind::NotFound,
            "gcloud token not available",
        )))
    }

    async fn get_token_from_service_account(
        credentials_path: &str,
    ) -> Result<String, ProviderError> {
        use std::fs;
        use tokio::time::{Duration, sleep};
        let creds_json = fs::read_to_string(credentials_path).map_err(ProviderError::IoError)?;
        let creds: ServiceAccountCreds =
            serde_json::from_str(&creds_json).map_err(|_| ProviderError::InvalidApiKey)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ProviderError::InvalidResponse("system time before Unix epoch".into()))?
            .as_secs();
        let header = base64_url_encode(&serde_json::json!({"alg": "RS256", "typ": "JWT"}));
        let claims = serde_json::json!({
            "iss": creds.client_email,
            "sub": creds.client_email,
            "aud": "https://oauth2.googleapis.com/token",
            "iat": now,
            "exp": now + 3600,
            "scope": "https://www.googleapis.com/auth/cloud-platform"
        });
        let claims_b64 = base64_url_encode(&claims);
        let signature = sign_rs256(&header, &claims_b64, &creds.private_key)?;
        let jwt = signature;
        let client = shared_client();
        let response = client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(ProviderError::RequestFailed)?;
        if !response.status().is_success() {
            return Err(ProviderError::HttpError(
                crate::error::HttpErrorDetail::new(
                    response.status().as_u16(),
                    response.text().await.unwrap_or_default(),
                ),
            ));
        }
        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(ProviderError::RequestFailed)?;
        sleep(Duration::from_secs(60 * 55)).await;
        Ok(token_response.access_token)
    }

    fn get_project_id() -> Result<String, ProviderError> {
        std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GOOGLE_PROJECT"))
            .map_err(|_| ProviderError::MissingApiKey)
    }

    fn get_region() -> String {
        std::env::var("GOOGLE_CLOUD_REGION").unwrap_or_else(|_| "us-central1".to_string())
    }
}

impl Default for VertexProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for VertexProvider {
    fn stream<'a>(
        &'a self,
        model: &'a Model,
        context: &'a Context,
        options: Option<StreamOptions>,
    ) -> Pin<Box<dyn Future<Output = StreamResult> + Send + 'a>> {
        Box::pin(async move {
            let options = options.unwrap_or_default();
            let access_token = self.get_access_token().await?;
            let project_id = Self::get_project_id()?;
            let region = Self::get_region();
            let model_id = &model.id;
            let url = format!(
                "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/google/models/{}:streamGenerateContent",
                region, project_id, region, model_id
            );
            let contents = convert_messages(context)?;
            let tools_json = convert_tools(&context.tools, false);
            let tool_config = super::google_shared::build_tool_config(options.tool_choice.as_ref());
            let body = build_request_body(
                &contents,
                context.system_prompt.as_deref(),
                tools_json.as_ref(),
                options.temperature,
                options.max_tokens,
                tool_config.as_ref(),
            );
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(ProviderError::RequestFailed)?;
            if !response.status().is_success() {
                let status = response.status();
                let body: String = response.text().await.unwrap_or_default();
                return Err(ProviderError::HttpError(
                    crate::error::HttpErrorDetail::new(status.as_u16(), body),
                ));
            }
            let model_name = model.id.clone();
            // Use split_complete_lines for safe UTF-8 boundary handling
            let stream = response
                .bytes_stream()
                .scan(
                    Vec::new(), // pending_bytes
                    move |pending_bytes, chunk: Result<bytes::Bytes, reqwest::Error>| {
                        let events = match chunk {
                            Ok(bytes) => {
                                let mut combined =
                                    Vec::with_capacity(pending_bytes.len() + bytes.len());
                                combined.extend_from_slice(pending_bytes);
                                combined.extend_from_slice(&bytes);
                                let (text, trailing) = split_complete_lines(&combined);
                                *pending_bytes = trailing;
                                parse_google_events(&text, Api::GoogleVertex, "vertex", &model_name)
                            }
                            Err(e) => vec![ProviderEvent::Error {
                                reason: StopReason::Error,
                                error: create_error_message(
                                    Api::GoogleVertex,
                                    "vertex",
                                    &e.to_string(),
                                ),
                            }],
                        };
                        async move { Some(futures::stream::iter(events)) }
                    },
                )
                .flatten();
            Ok(Box::pin(stream) as Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>)
        })
    }
}

fn base64_url_encode(value: &serde_json::Value) -> String {
    use base64::Engine as _;
    // SAFETY: `serde_json::to_string` on a `Value` cannot fail — the Value type
    // has no custom serializer that returns Err. Infallible by construction.
    #[allow(clippy::expect_used)]
    let json = serde_json::to_string(value).expect("serializing json value");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes())
}

fn sign_rs256(
    header_b64: &str,
    claims_b64: &str,
    private_key_pem: &str,
) -> Result<String, ProviderError> {
    use base64::Engine as _;
    use pkcs8::DecodePrivateKey;
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use sha2::Sha256;
    use signature::{SignatureEncoding, Signer};
    let message = format!("{}.{}", header_b64, claims_b64);
    let key =
        RsaPrivateKey::from_pkcs8_pem(private_key_pem).map_err(|_| ProviderError::InvalidApiKey)?;
    let signing_key = SigningKey::<Sha256>::new_unprefixed(key);
    let signature = signing_key.sign(message.as_bytes());
    let sig_bytes = signature.to_bytes();
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sig_bytes);
    Ok(format!("{}.{}", message, sig_b64))
}

#[derive(Debug, serde::Deserialize)]
// serde deserialization structs
struct TokenResponse {
    access_token: String,
    _expires_in: usize,
    _token_type: String,
}

#[derive(Debug, serde::Deserialize)]
// serde deserialization structs
struct ServiceAccountCreds {
    #[serde(rename = "type")]
    _type: String,
    _project_id: String,
    _private_key_id: String,
    private_key: String,
    client_email: String,
    _client_id: String,
    _auth_uri: String,
    _token_uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Message};

    #[test]
    fn test_build_vertex_contents_with_text() {
        let mut ctx = Context::new();
        ctx.add_message(Message::user("Hello, world!"));
        let contents = convert_messages(&ctx).unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "Hello, world!");
    }

    #[test]
    fn test_build_vertex_tools() {
        let tools = vec![crate::Tool::new(
            "get_weather",
            "Get weather for a location",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string", "description": "The city name" }
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
    fn test_parse_vertex_events_basic_text() {
        let sse_data = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;
        let events = parse_google_events(sse_data, Api::GoogleVertex, "vertex", "gemini-1.5-pro");
        assert!(!events.is_empty());
        if let ProviderEvent::TextDelta { delta, .. } = &events[0] {
            assert_eq!(delta, "Hello");
        } else {
            panic!("Expected TextDelta event");
        }
    }

    #[test]
    fn test_get_region_default() {
        // SAFETY: test-only, single-threaded test binary.
        unsafe { std::env::remove_var("GOOGLE_CLOUD_REGION") };
        assert_eq!(VertexProvider::get_region(), "us-central1");
    }

    #[test]
    fn test_create_error_message() {
        let msg = create_error_message(Api::GoogleVertex, "vertex", "Something went wrong");
        assert_eq!(msg.provider, "vertex");
        assert_eq!(msg.api, Api::GoogleVertex);
        assert_eq!(msg.stop_reason, StopReason::Error);
    }
}
