//! File-based `ConfigStore` — single TOML file with dotted-key flattening.

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::path::PathBuf;

use oxi_sdk::ports::{ConfigStore, PortValue};
use oxi_sdk::SdkError;

/// File-based config. Reads from `path` on construction and writes back on
/// every `set`.
pub struct FileConfigStore {
    path: PathBuf,
    state: RwLock<BTreeMap<String, JsonValue>>,
}

impl std::fmt::Debug for FileConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileConfigStore").field("path", &self.path).finish()
    }
}

impl FileConfigStore {
    /// Create a config store backed by `path`. If the file does not exist,
    /// an empty store is returned; the file is created on the first `set`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = Self::load(&path);
        Self {
            path,
            state: RwLock::new(state),
        }
    }

    fn load(path: &std::path::Path) -> BTreeMap<String, JsonValue> {
        if !path.exists() {
            return BTreeMap::new();
        }
        match std::fs::read_to_string(path) {
            Ok(text) => toml_to_flat_map(&text).unwrap_or_default(),
            Err(_) => BTreeMap::new(),
        }
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let snapshot = self.state.read().clone();
        let nested = flat_map_to_nested(&snapshot);
        let text = toml::to_string_pretty(&nested).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Convert a TOML document into a flat dotted-key map.
fn toml_to_flat_map(text: &str) -> Result<BTreeMap<String, JsonValue>, toml::de::Error> {
    let v: toml::Value = toml::from_str(text)?;
    let mut out = BTreeMap::new();
    flatten_into(&v, "", &mut out);
    Ok(out)
}

fn flatten_into(v: &toml::Value, prefix: &str, out: &mut BTreeMap<String, JsonValue>) {
    match v {
        toml::Value::Table(t) => {
            for (k, vv) in t {
                let next = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_into(vv, &next, out);
            }
        }
        toml::Value::Array(a) => {
            let j = serde_json::to_value(a).unwrap_or(JsonValue::Null);
            out.insert(prefix.to_string(), j);
        }
        other => {
            let j = serde_json::to_value(other).unwrap_or(JsonValue::Null);
            out.insert(prefix.to_string(), j);
        }
    }
}

fn flat_map_to_nested(flat: &BTreeMap<String, JsonValue>) -> toml::Value {
    let mut root = toml::value::Table::new();
    for (key, value) in flat {
        let parts: Vec<&str> = key.split('.').collect();
        insert_nested(&mut root, &parts, value.clone());
    }
    toml::Value::Table(root)
}

fn insert_nested(root: &mut toml::value::Table, parts: &[&str], value: JsonValue) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        root.insert(parts[0].to_string(), json_to_toml(value));
        return;
    }
    let head = parts[0];
    let entry = root
        .entry(head.to_string())
        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
    if let toml::Value::Table(t) = entry {
        insert_nested(t, &parts[1..], value);
    }
}

fn json_to_toml(v: JsonValue) -> toml::Value {
    match v {
        JsonValue::Null => toml::Value::String(String::new()),
        JsonValue::Bool(b) => toml::Value::Boolean(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        JsonValue::String(s) => toml::Value::String(s),
        JsonValue::Array(a) => toml::Value::Array(
            a.into_iter().map(json_to_toml).collect(),
        ),
        JsonValue::Object(o) => {
            let mut t = toml::value::Table::new();
            for (k, v) in o {
                t.insert(k, json_to_toml(v));
            }
            toml::Value::Table(t)
        }
    }
}

#[async_trait]
impl ConfigStore for FileConfigStore {
    fn get(&self, key: &str) -> Result<Option<PortValue>, SdkError> {
        Ok(self.state.read().get(key).cloned())
    }

    fn set(&self, key: &str, value: PortValue) -> Result<(), SdkError> {
        {
            let mut s = self.state.write();
            s.insert(key.to_string(), value);
        }
        self.save().map_err(|e| SdkError::Internal(e.into()))
    }

    fn list(&self) -> Result<Vec<(String, PortValue)>, SdkError> {
        Ok(self.state.read().iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn round_trip_nested_keys() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("settings.toml");
        let c = FileConfigStore::new(&p);
        c.set("model.provider", json!("anthropic")).unwrap();
        c.set("model.name", json!("claude-sonnet-4-20250514")).unwrap();
        c.set("ui.theme", json!("dark")).unwrap();
        // Reload from disk.
        let c2 = FileConfigStore::new(&p);
        assert_eq!(
            c2.get("model.provider").unwrap(),
            Some(json!("anthropic"))
        );
        assert_eq!(c2.get("ui.theme").unwrap(), Some(json!("dark")));
    }

    #[test]
    fn get_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let c = FileConfigStore::new(tmp.path().join("nope.toml"));
        assert!(c.get("any").unwrap().is_none());
    }
}
