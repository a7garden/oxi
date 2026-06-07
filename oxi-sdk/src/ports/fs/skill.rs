//! File-based `SkillLoader` — discovers `SKILL.md` files under a root directory.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use crate::SdkError;
use crate::ports::{Skill, SkillLoader, SkillMeta};

/// Discovers `SKILL.md` files under one or more root directories.
///
/// Layout:
/// ```text
/// <root>/
///   <skill-name>/
///     SKILL.md
/// ```
///
/// `SKILL.md` is parsed: lines 1..N are YAML frontmatter (delimited by `---`),
/// the remainder is the body.
pub struct FileSkillLoader {
    roots: Vec<PathBuf>,
}

impl std::fmt::Debug for FileSkillLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSkillLoader")
            .field("roots", &self.roots)
            .finish()
    }
}

impl FileSkillLoader {
    /// Create a loader that scans the given root(s).
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            roots: roots.into_iter().collect(),
        }
    }

    /// Convenience: scan a single root directory.
    pub fn single(root: impl Into<PathBuf>) -> Self {
        Self::new(vec![root.into()])
    }
}

impl SkillLoader for FileSkillLoader {
    fn list(&self) -> Pin<Box<dyn Future<Output = Result<Vec<SkillMeta>, SdkError>> + Send + '_>> {
        let mut out = Vec::new();
        for root in &self.roots {
            if !root.exists() {
                continue;
            }
            let entries = match std::fs::read_dir(root) {
                Ok(e) => e,
                Err(e) => return Box::pin(async { Err(scan_err(e)) }),
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let skill_md = path.join("SKILL.md");
                if !skill_md.exists() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if let Ok(meta) = parse_meta(name, &skill_md) {
                    out.push(meta);
                }
            }
        }
        Box::pin(async { Ok(out) })
    }

    fn load(
        &self,
        name: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Skill>, SdkError>> + Send + '_>> {
        for root in &self.roots {
            let path = root.join(name).join("SKILL.md");
            if path.exists() {
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => return Box::pin(async { Err(read_err(e)) }),
                };
                let meta = match parse_meta(name, &path) {
                    Ok(m) => m,
                    Err(e) => return Box::pin(async { Err(e) }),
                };
                let body = strip_frontmatter(&text);
                return Box::pin(async { Ok(Some(Skill { meta, body })) });
            }
        }
        Box::pin(async { Ok(None) })
    }
}

fn parse_meta(name: &str, path: &Path) -> Result<SkillMeta, SdkError> {
    let text = std::fs::read_to_string(path).map_err(read_err)?;
    let mut description = String::new();
    let mut version = None;
    if let Some(body) = text.strip_prefix("---\n")
        && let Some(end) = body.find("\n---")
    {
        let fm = &body[..end];
        for line in fm.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("description:") {
                description = rest.trim().trim_matches('"').to_string();
            } else if let Some(rest) = line.strip_prefix("version:") {
                version = Some(rest.trim().trim_matches('"').to_string());
            }
        }
    }
    Ok(SkillMeta {
        name: name.to_string(),
        description,
        path: path.to_path_buf(),
        version,
    })
}

fn strip_frontmatter(text: &str) -> String {
    if let Some(body) = text.strip_prefix("---\n")
        && let Some(idx) = body.find("\n---")
    {
        let after = &body[idx + 4..];
        return after.trim_start_matches('\n').to_string();
    }
    text.to_string()
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
    async fn discovers_skill_md() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("git-commit");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: write a commit\nversion: \"1.0\"\n---\n# body\nhello",
        )
        .unwrap();
        let loader = FileSkillLoader::single(tmp.path());
        let list = loader.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "git-commit");
        assert_eq!(list[0].description, "write a commit");
    }

    #[tokio::test]
    async fn load_returns_body() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: code review\n---\nreview the diff",
        )
        .unwrap();
        let loader = FileSkillLoader::single(tmp.path());
        let s = loader.load("review").await.unwrap().unwrap();
        assert!(s.body.contains("review the diff"));
    }

    #[tokio::test]
    async fn load_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let loader = FileSkillLoader::single(tmp.path());
        assert!(loader.load("absent").await.unwrap().is_none());
    }
}
