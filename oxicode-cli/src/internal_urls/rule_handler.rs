//! `rule://` protocol handler — resolves TTSR rule names to their content.

use std::sync::Arc;

use async_trait::async_trait;
use oxicode_sdk::SdkError;
use oxicode_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl, RuleRegistry};

/// Protocol handler for `rule://` URLs backed by the SDK's `RuleRegistry` port.
pub struct RuleProtocolHandler {
    registry: Arc<dyn RuleRegistry>,
}

impl RuleProtocolHandler {
    /// Create a new handler backed by the given rule registry.
    pub fn new(registry: Arc<dyn RuleRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ProtocolHandler for RuleProtocolHandler {
    fn scheme(&self) -> &str {
        "rule"
    }
    fn immutable(&self) -> bool {
        true
    }

    async fn resolve(
        &self,
        url: &str,
        _selector: Option<&str>,
        _ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError> {
        let rule_name = url
            .strip_prefix("rule://")
            .unwrap_or(url)
            .trim_end_matches('/');
        if rule_name.is_empty() {
            return Err(SdkError::ExecutionFailed {
                reason: "rule:// URL requires a rule name".into(),
            });
        }

        let rules = self.registry.rules().await;
        let rule = rules.iter().find(|r| r.name == rule_name).ok_or_else(|| {
            let available: Vec<String> = rules.iter().map(|r| r.name.clone()).collect();
            SdkError::ExecutionFailed {
                reason: format!(
                    "Unknown rule: '{rule_name}'. Available: {}",
                    if available.is_empty() {
                        "none".to_string()
                    } else {
                        available.join(", ")
                    }
                ),
            }
        })?;

        Ok(ResolvedUrl {
            url: format!("rule://{rule_name}"),
            content: rule.content.clone(),
            content_type: "text/markdown".into(),
            size: None,
            source_path: None,
            notes: vec![],
            immutable: true,
        })
    }
}
