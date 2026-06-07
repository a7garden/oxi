//! File-based `PersonaProvider` — discover persona files from a directory.
//!
//! Layout: `<root>/<name>.md` with optional frontmatter:
//!
//! ```markdown
//! ---
//! preferred_model: anthropic/claude-sonnet-4-20250514
//! allowed_tools:
//!   - read
//!   - grep
//! ---
//! You are a senior reviewer focused on security and correctness.
//! ```
//!
//! The system prompt body is everything after the closing `---`.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::SdkError;
use crate::ports::{Persona, PersonaProvider};

/// Discovers `<root>/<name>.md` persona files and parses them.
pub struct FilePersonaProvider {
    root: PathBuf,
}

impl std::fmt::Debug for FilePersonaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilePersonaProvider")
            .field("root", &self.root)
            .finish()
    }
}

impl FilePersonaProvider {
    /// Scan `root` for `<name>.md` persona files.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl PersonaProvider for FilePersonaProvider {
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<Persona>, SdkError>> + Send + '_>> {
        if !self.root.exists() {
            return Box::pin(async { Ok(Vec::new()) });
        }
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) => return Box::pin(async { Err(scan_err(e)) }),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|n| n.to_str())
                && let Ok(p) = parse_persona(name, &path)
            {
                out.push(p);
            }
        }
        Box::pin(async { Ok(out) })
    }

    fn get(
        &self,
        name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Persona>, SdkError>> + Send + '_>> {
        let path = self.root.join(format!("{name}.md"));
        if !path.exists() {
            return Box::pin(async { Ok(None) });
        }
        match parse_persona(name, &path) {
            Ok(p) => Box::pin(async { Ok(Some(p)) }),
            Err(e) => Box::pin(async { Err(e) }),
        }
    }
}

/// Parse a persona file. Returns (front_matter, body).
fn split_frontmatter(text: &str) -> (Option<String>, String) {
    if let Some(body) = text.strip_prefix("---\n")
        && let Some(idx) = body.find("\n---")
    {
        let fm = body[..idx].to_string();
        let after = &body[idx + 4..];
        return (Some(fm), after.trim_start_matches('\n').to_string());
    }
    (None, text.to_string())
}

fn parse_persona(name: &str, path: &std::path::Path) -> Result<Persona, SdkError> {
    let text = std::fs::read_to_string(path).map_err(read_err)?;
    let (front, body) = split_frontmatter(&text);

    let mut preferred_model = None;
    let mut allowed_tools = None;

    if let Some(fm) = front {
        for line in fm.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("preferred_model:") {
                preferred_model = Some(rest.trim().trim_matches('"').to_string());
            } else if let Some(rest) = line.strip_prefix("allowed_tools:") {
                let items: Vec<String> = rest
                    .trim()
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !items.is_empty() {
                    allowed_tools = Some(items);
                }
            }
        }
    }

    Ok(Persona {
        name: name.to_string(),
        system_prompt: body,
        preferred_model,
        allowed_tools,
    })
}

fn read_err(e: std::io::Error) -> SdkError {
    SdkError::Internal(anyhow::anyhow!(e))
}
fn scan_err(e: std::io::Error) -> SdkError {
    SdkError::Internal(anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn discovers_persona_with_frontmatter() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("reviewer.md"),
            "---\npreferred_model: anthropic/claude-sonnet-4-20250514\n---\nYou are a reviewer.",
        )
        .unwrap();
        let p = FilePersonaProvider::new(tmp.path());
        let list = p.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "reviewer");
        assert_eq!(
            list[0].preferred_model.as_deref(),
            Some("anthropic/claude-sonnet-4-20250514")
        );
        assert!(list[0].system_prompt.contains("You are a reviewer"));
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let p = FilePersonaProvider::new(tmp.path());
        assert!(p.get("absent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn persona_without_frontmatter_uses_full_body() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("plain.md"), "Just a system prompt.").unwrap();
        let p = FilePersonaProvider::new(tmp.path());
        let got = p.get("plain").await.unwrap().unwrap();
        assert!(got.system_prompt.contains("Just a system prompt"));
        assert!(got.preferred_model.is_none());
    }
}
