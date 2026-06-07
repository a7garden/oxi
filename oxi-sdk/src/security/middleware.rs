//! SecurityMiddleware — tool execution authorization via the Middleware trait.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::middleware::{
    Middleware, MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewareResult,
};
use crate::security::authorizer::Authorizer;
use crate::security::capability::{Capability, CapabilitySubject};

/// Middleware that checks tool execution against the Authorizer.
///
/// Installed in the `BeforeTool` phase. For each tool call:
/// 1. Infers the required capability from tool name + params.
/// 2. Checks with the Authorizer.
/// 3. Blocks if denied.
pub struct SecurityMiddleware {
    authorizer: Arc<Authorizer>,
}

impl SecurityMiddleware {
    /// Create a new security middleware with the given authorizer.
    pub fn new(authorizer: Arc<Authorizer>) -> Self {
        Self { authorizer }
    }

    /// Infer the capability required for a tool call from its name and params.
    pub fn required_capability(tool_name: &str, params: &serde_json::Value) -> Option<Capability> {
        match tool_name {
            "read" => params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| Capability::FileRead {
                    path_pattern: p.to_string(),
                }),
            "write" => params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| Capability::FileWrite {
                    path_pattern: p.to_string(),
                }),
            "edit" => params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| Capability::FileEdit {
                    path_pattern: p.to_string(),
                }),
            "ls" => params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| Capability::FileList {
                    path_pattern: p.to_string(),
                }),
            "find" => params
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| Capability::FileFind {
                    path_pattern: p.to_string(),
                }),
            "bash" => {
                let cmd = params
                    .get("command")
                    .or_else(|| params.get("cmd"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let first_word = cmd.split_whitespace().next().unwrap_or("").to_string();
                Some(Capability::Bash {
                    allowed_commands: vec![crate::security::capability::StringPattern::Literal(
                        first_word,
                    )],
                    timeout_secs: None,
                })
            }
            "browse" | "browse_extract" => {
                let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("*");
                let domain = extract_domain(url);
                Some(Capability::WebBrowse {
                    allowed_domains: vec![domain],
                })
            }
            "web_search" => Some(Capability::Network {
                allowed_domains: vec!["*".into()],
            }),
            "subagent" => Some(Capability::Subagent { max_children: None }),
            _ => Some(Capability::ToolUse {
                tool_name: tool_name.to_string(),
            }),
        }
    }
}

/// Extract domain from a URL string.
fn extract_domain(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("*")
        .to_string()
}

impl Middleware for SecurityMiddleware {
    fn name(&self) -> &str {
        "security"
    }

    fn phases(&self) -> Vec<MiddlewarePhase> {
        vec![MiddlewarePhase::BeforeTool]
    }

    fn handle<'a>(
        &'a self,
        ctx: &'a MiddlewareContext,
    ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
        Box::pin(async move {
            if let MiddlewareData::BeforeTool { tool_name, params } = &ctx.data {
                let subject = CapabilitySubject::Agent(ctx.agent_id.clone());
                if let Some(required) = Self::required_capability(tool_name, params)
                    && !self.authorizer.check(&subject, &required)
                {
                    return MiddlewareResult::block(format!(
                        "Permission denied for agent {}: {:?}",
                        ctx.agent_id, required
                    ));
                }
            }
            MiddlewareResult::pass()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::AuditLog;
    use crate::security::capability::CapabilitySet;
    use crate::security::capability::StringPattern;

    fn test_mw() -> SecurityMiddleware {
        let auth = Arc::new(Authorizer::new(Arc::new(AuditLog::new(64))));
        SecurityMiddleware::new(auth)
    }

    fn test_mw_with_coding() -> (SecurityMiddleware, Arc<Authorizer>) {
        let auth = Arc::new(Authorizer::new(Arc::new(AuditLog::new(64))));
        auth.grant(
            CapabilitySubject::Agent("a1".into()),
            CapabilitySet::coding("/workspace"),
        );
        (SecurityMiddleware::new(Arc::clone(&auth)), auth)
    }

    #[test]
    fn infer_capability_read() {
        let cap = SecurityMiddleware::required_capability(
            "read",
            &serde_json::json!({"path": "/ws/file.rs"}),
        );
        assert!(matches!(cap, Some(Capability::FileRead { .. })));
    }

    #[test]
    fn infer_capability_bash() {
        let cap = SecurityMiddleware::required_capability(
            "bash",
            &serde_json::json!({"command": "git status"}),
        );
        match cap {
            Some(Capability::Bash {
                allowed_commands, ..
            }) => {
                assert!(
                    matches!(&allowed_commands[0], StringPattern::Literal(cmd) if cmd == "git")
                );
            }
            _ => panic!("Expected Bash capability"),
        }
    }

    #[test]
    fn infer_capability_unknown_tool() {
        let cap = SecurityMiddleware::required_capability("custom_tool", &serde_json::json!({}));
        assert!(matches!(cap, Some(Capability::ToolUse { .. })));
    }

    #[tokio::test]
    async fn allows_authorized_tool() {
        let (mw, _) = test_mw_with_coding();
        let ctx = MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "a1",
            MiddlewareData::BeforeTool {
                tool_name: "read".into(),
                params: serde_json::json!({"path": "/workspace/src/main.rs"}),
            },
        );
        let result = mw.handle(&ctx).await;
        assert!(result.is_continue());
    }

    #[tokio::test]
    async fn blocks_unauthorized_tool() {
        let (mw, _) = test_mw_with_coding();
        let ctx = MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "a1",
            MiddlewareData::BeforeTool {
                tool_name: "bash".into(),
                params: serde_json::json!({"command": "rm -rf /"}),
            },
        );
        let result = mw.handle(&ctx).await;
        assert!(result.is_block());
    }

    #[tokio::test]
    async fn blocks_agent_without_grants() {
        let mw = test_mw();
        let ctx = MiddlewareContext::new(
            MiddlewarePhase::BeforeTool,
            "unknown-agent",
            MiddlewareData::BeforeTool {
                tool_name: "read".into(),
                params: serde_json::json!({"path": "/any/file"}),
            },
        );
        let result = mw.handle(&ctx).await;
        assert!(result.is_block());
    }

    #[test]
    fn extract_domain_test() {
        assert_eq!(extract_domain("https://example.com/path"), "example.com");
        assert_eq!(extract_domain("http://sub.example.com/"), "sub.example.com");
        assert_eq!(extract_domain("not-a-url"), "not-a-url");
    }
}
