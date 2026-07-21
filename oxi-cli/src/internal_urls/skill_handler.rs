//! `skill://` protocol handler — resolves skill names to SKILL.md content.

use std::sync::Arc;

use async_trait::async_trait;
use oxi_sdk::SdkError;
use oxi_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl};

/// Protocol handler for `skill://` URLs backed by the SDK's `SkillLoader` port.
pub struct SkillProtocolHandler {
    loader: Arc<dyn oxi_sdk::ports::SkillLoader>,
}

impl SkillProtocolHandler {
    /// Create a new handler backed by the given skill loader.
    pub fn new(loader: Arc<dyn oxi_sdk::ports::SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl ProtocolHandler for SkillProtocolHandler {
    fn scheme(&self) -> &str {
        "skill"
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
        let suffix = url.strip_prefix("skill://").unwrap_or(url);
        let parts: Vec<&str> = suffix.splitn(2, '/').collect();
        let skill_name = parts[0];
        if skill_name.is_empty() {
            return Err(SdkError::ExecutionFailed {
                reason: "skill:// URL requires a skill name".into(),
            });
        }

        let skills = self.loader.list().await?;
        let skill = skills
            .iter()
            .find(|s| s.name == skill_name)
            .ok_or_else(|| {
                let available: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
                SdkError::ExecutionFailed {
                    reason: format!(
                        "Unknown skill: '{skill_name}'. Available: {}",
                        available.join(", ")
                    ),
                }
            })?;

        let target_file = parts.get(1).copied().unwrap_or("SKILL.md");
        if target_file.starts_with('/') || target_file.contains("..") {
            return Err(SdkError::ExecutionFailed {
                reason: "Path traversal is not allowed in skill:// URLs".into(),
            });
        }

        let skill_dir = skill.path.parent().unwrap_or(std::path::Path::new(""));
        let file_path = skill_dir.join(target_file);

        let content =
            tokio::fs::read_to_string(&file_path)
                .await
                .map_err(|e| SdkError::ExecutionFailed {
                    reason: format!("Failed to read skill file '{}': {e}", file_path.display()),
                })?;

        Ok(ResolvedUrl {
            url: format!("skill://{suffix}"),
            content,
            content_type: if target_file.ends_with(".md") {
                "text/markdown".into()
            } else {
                "text/plain".into()
            },
            size: None,
            source_path: Some(file_path.to_string_lossy().into_owned()),
            notes: vec![],
            immutable: true,
        })
    }
}
