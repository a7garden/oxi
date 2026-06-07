//! Plugin loader for dynamic middleware loading

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::Middleware;

/// Plugin manifest — metadata for a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub phases: Vec<String>,
    pub entry_point: String,
    pub permissions: Vec<String>,
}

impl PluginManifest {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

pub struct PluginLoader {
    #[expect(dead_code)]
    plugins_dir: PathBuf,
    loaded: Arc<RwLock<HashMap<String, Arc<dyn Middleware>>>>,
    manifests: Arc<RwLock<HashMap<String, PluginManifest>>>,
}

impl PluginLoader {
    pub fn new(plugins_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins_dir: plugins_dir.into(),
            loaded: Arc::new(RwLock::new(HashMap::new())),
            manifests: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn load(&self, manifest_path: &Path) -> anyhow::Result<String> {
        let manifest = PluginManifest::from_file(manifest_path)
            .map_err(|e| anyhow::anyhow!("Failed to load manifest: {}", e))?;
        let name = manifest.name.clone();
        let mut manifests = self.manifests.write();
        manifests.insert(name.clone(), manifest);
        Ok(name)
    }

    pub fn middlewares(&self) -> Vec<Arc<dyn Middleware>> {
        let loaded = self.loaded.read();
        loaded.values().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Middleware>> {
        self.loaded.read().get(name).cloned()
    }

    pub fn unload(&self, name: &str) -> bool {
        let mut loaded = self.loaded.write();
        let removed = loaded.remove(name).is_some();
        let mut manifests = self.manifests.write();
        manifests.remove(name);
        removed
    }

    pub fn register(&self, middleware: Arc<dyn Middleware>) {
        let mut loaded = self.loaded.write();
        loaded.insert(middleware.name().to_string(), middleware);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Middleware;
    use crate::middleware::{MiddlewareContext, MiddlewareData, MiddlewarePhase, MiddlewareResult};
    use std::future::Future;
    use std::pin::Pin;

    struct MockMiddleware {
        name: String,
    }

    impl MockMiddleware {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    impl Middleware for MockMiddleware {
        fn name(&self) -> &str {
            &self.name
        }
        fn phases(&self) -> Vec<MiddlewarePhase> {
            vec![MiddlewarePhase::BeforeTool]
        }
        fn handle<'a>(
            &'a self,
            _ctx: &'a MiddlewareContext,
        ) -> Pin<Box<dyn Future<Output = MiddlewareResult> + Send + 'a>> {
            Box::pin(async { MiddlewareResult::pass() })
        }
    }

    #[test]
    fn test_plugin_loader_register() {
        let loader = PluginLoader::new("/tmp/plugins");
        loader.register(Arc::new(MockMiddleware::new("test-plugin")));
        let mws = loader.middlewares();
        assert_eq!(mws.len(), 1);
        assert_eq!(mws[0].name(), "test-plugin");
    }

    #[test]
    fn test_plugin_loader_unload() {
        let loader = PluginLoader::new("/tmp/plugins");
        loader.register(Arc::new(MockMiddleware::new("test-plugin")));
        assert!(loader.get("test-plugin").is_some());
        loader.unload("test-plugin");
        assert!(loader.get("test-plugin").is_none());
    }

    #[test]
    fn test_plugin_manifest_parse() {
        let json = r#"{"name":"test-plugin","version":"1.0.0","phases":["before_tool"],"entry_point":"libtest.so","permissions":[]}"#;
        let manifest = PluginManifest::from_json(json).unwrap();
        assert_eq!(manifest.name, "test-plugin");
    }
}
