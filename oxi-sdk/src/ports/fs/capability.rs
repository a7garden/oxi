//! Rule-based `CapabilityResolver` — TOML config maps subjects to tool lists.

use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::ports::CapabilityResolver;
use crate::SdkError;

/// Resolves visible tools per subject from a TOML file:
///
/// ```toml
/// [subjects]
/// "agent-1" = ["read", "grep", "ls"]
/// "agent-2" = ["read", "write", "edit", "bash"]
/// "agent-*" = ["read"]   # default — wildcard suffix
/// ```
pub struct TomlCapabilityResolver {
    path: Option<PathBuf>,
    subjects: RwLock<BTreeMap<String, Vec<String>>>,
}

impl std::fmt::Debug for TomlCapabilityResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TomlCapabilityResolver")
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct RulesFile {
    #[serde(default)]
    subjects: BTreeMap<String, Vec<String>>,
}

impl TomlCapabilityResolver {
    /// Create a resolver that allows no tools.
    pub fn empty() -> Self {
        Self {
            path: None,
            subjects: RwLock::new(BTreeMap::new()),
        }
    }

    /// Load rules from a TOML file. Missing file = empty rules.
    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let subjects = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str::<RulesFile>(&s).ok())
                .map(|f| f.subjects)
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        Self {
            path: Some(path),
            subjects: RwLock::new(subjects),
        }
    }

    pub fn reload(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = std::fs::read_to_string(path)?;
        let parsed: RulesFile = toml::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        *self.subjects.write() = parsed.subjects;
        Ok(())
    }
}

#[async_trait]
impl CapabilityResolver for TomlCapabilityResolver {
    async fn visible_tools(&self, subject: &str) -> Result<Vec<String>, SdkError> {
        let g = self.subjects.read();
        // Exact match first.
        if let Some(list) = g.get(subject) {
            return Ok(list.clone());
        }
        // Wildcard suffix: keys ending with `*` are defaults.
        // Order: most specific prefix wins.
        let mut best: Option<&Vec<String>> = None;
        for (key, list) in g.iter() {
            if let Some(prefix) = key.strip_suffix('*') {
                if subject.starts_with(prefix)
                    && (best.is_none() || prefix.len() > key.trim_end_matches('*').len())
                {
                    best = Some(list);
                }
            }
        }
        Ok(best.cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn empty_resolver_returns_nothing() {
        let r = TomlCapabilityResolver::empty();
        assert!(r.visible_tools("anyone").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn exact_match() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("caps.toml");
        fs::write(
            &p,
            r#"[subjects]
"agent-1" = ["read", "write"]
"#,
        )
        .unwrap();
        let r = TomlCapabilityResolver::from_file(&p);
        let v = r.visible_tools("agent-1").await.unwrap();
        assert_eq!(v, vec!["read", "write"]);
    }

    #[tokio::test]
    async fn wildcard_default() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("caps.toml");
        fs::write(
            &p,
            r#"[subjects]
"agent-*" = ["read"]
"#,
        )
        .unwrap();
        let r = TomlCapabilityResolver::from_file(&p);
        let v = r.visible_tools("agent-anything").await.unwrap();
        assert_eq!(v, vec!["read"]);
    }
}
