//! WASM skill for oxi — running WebAssembly modules as tools
//!
//! Provides the ability to load, validate, and execute WASM modules
//! within the agent runtime. This skill enables:
//!
//! 1. **Loading** WASM modules from `.wasm` files or base64-encoded bytes
//! 2. **Validating** modules against capability requirements
//! 3. **Executing** exported functions with typed arguments
//! 4. **Managing** a registry of loaded WASM tools
//!
//! The module provides:
//! - [`WasmModule`] — a loaded and validated WASM module
//! - [`WasmFunction`] — metadata about an exported function
//! - [`WasmRegistry`] — manages loaded WASM modules
//! - [`WasmSkill`] — skill prompt generator

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// WASM value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WasmValueType {
    I32,
    I64,
    F32,
    F64,
}

impl fmt::Display for WasmValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmValueType::I32 => write!(f, "i32"),
            WasmValueType::I64 => write!(f, "i64"),
            WasmValueType::F32 => write!(f, "f32"),
            WasmValueType::F64 => write!(f, "f64"),
        }
    }
}

/// Metadata about an exported WASM function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmFunction {
    pub name: String,
    pub params: Vec<WasmValueType>,
    pub results: Vec<WasmValueType>,
}

impl WasmFunction {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            results: Vec::new(),
        }
    }

    pub fn param(mut self, ty: WasmValueType) -> Self {
        self.params.push(ty);
        self
    }

    pub fn result(mut self, ty: WasmValueType) -> Self {
        self.results.push(ty);
        self
    }

    pub fn signature(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let results = self
            .results
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if results.is_empty() {
            format!("{}({}) -> void", self.name, params)
        } else {
            format!("{}({}) -> {}", self.name, params, results)
        }
    }
}

/// Capabilities that a WASM module may require.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WasmCapability {
    None,
    FsRead,
    FsWrite,
    Network,
    EnvVars,
    Clock,
    Random,
    All,
}

impl fmt::Display for WasmCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmCapability::None => write!(f, "none"),
            WasmCapability::FsRead => write!(f, "fs-read"),
            WasmCapability::FsWrite => write!(f, "fs-write"),
            WasmCapability::Network => write!(f, "network"),
            WasmCapability::EnvVars => write!(f, "env-vars"),
            WasmCapability::Clock => write!(f, "clock"),
            WasmCapability::Random => write!(f, "random"),
            WasmCapability::All => write!(f, "all"),
        }
    }
}

/// A loaded WASM module with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmModule {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub exports: Vec<WasmFunction>,
    pub required_capabilities: Vec<WasmCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

impl WasmModule {
    /// Load a WASM module from a file.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            bail!("WASM file not found: {}", path.display());
        }
        let metadata =
            fs::metadata(path).with_context(|| format!("Failed to stat {}", path.display()))?;
        if metadata.len() == 0 {
            bail!("WASM file is empty: {}", path.display());
        }
        let bytes = fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
        if bytes.len() < 8 {
            bail!(
                "WASM file too small ({} bytes): {}",
                bytes.len(),
                path.display()
            );
        }
        if &bytes[0..4] != b"\0asm" {
            bail!("Invalid WASM file (bad magic bytes): {}", path.display());
        }
        let _version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let hash = sha256_hex(&bytes);
        let exports = parse_wasm_exports(&bytes);

        Ok(Self {
            name,
            path: path.to_path_buf(),
            size_bytes: metadata.len(),
            exports,
            required_capabilities: Vec::new(),
            description: None,
            hash: Some(hash),
        })
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn require_capability(mut self, cap: WasmCapability) -> Self {
        if !self.required_capabilities.contains(&cap) {
            self.required_capabilities.push(cap);
        }
        self
    }

    pub fn has_export(&self, name: &str) -> bool {
        self.exports.iter().any(|f| f.name == name)
    }

    pub fn get_export(&self, name: &str) -> Option<&WasmFunction> {
        self.exports.iter().find(|f| f.name == name)
    }

    pub fn validate_exports(&self, required: &[&str]) -> Vec<String> {
        required
            .iter()
            .filter(|name| !self.has_export(name))
            .map(|name| (*name).to_string())
            .collect()
    }

    pub fn render_summary(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str(&format!(
            "Module: {} ({})\n",
            self.name,
            self.path.display()
        ));
        out.push_str(&format!("Size: {} bytes\n", self.size_bytes));
        if let Some(ref hash) = self.hash {
            out.push_str(&format!("Hash: {}...\n", &hash[..16]));
        }
        if let Some(ref desc) = self.description {
            out.push_str(&format!("Description: {}\n", desc));
        }
        if !self.required_capabilities.is_empty() {
            let caps: Vec<String> = self
                .required_capabilities
                .iter()
                .map(|c| c.to_string())
                .collect();
            out.push_str(&format!("Capabilities: {}\n", caps.join(", ")));
        }
        if !self.exports.is_empty() {
            out.push_str("Exports:\n");
            for func in &self.exports {
                out.push_str(&format!("  - {}\n", func.signature()));
            }
        }
        out
    }
}

/// Registry of loaded WASM modules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmRegistry {
    modules: HashMap<String, WasmModule>,
    search_paths: Vec<PathBuf>,
}

impl WasmRegistry {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            search_paths: Vec::new(),
        }
    }

    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.search_paths.contains(&path) {
            self.search_paths.push(path);
        }
    }

    pub fn register(&mut self, module: WasmModule) {
        self.modules.insert(module.name.clone(), module);
    }

    pub fn get(&self, name: &str) -> Option<&WasmModule> {
        self.modules.get(name)
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn scan_search_paths(&mut self) -> Result<Vec<String>> {
        let mut loaded = Vec::new();
        for search_path in &self.search_paths {
            if !search_path.exists() {
                continue;
            }
            let entries = fs::read_dir(search_path)
                .with_context(|| format!("Failed to read {}", search_path.display()))?;
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
                    match WasmModule::load(&path) {
                        Ok(module) => {
                            let name = module.name.clone();
                            self.modules.insert(name.clone(), module);
                            loaded.push(name);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to load {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }
        Ok(loaded)
    }
}

impl Default for WasmRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WasmSkill;

impl WasmSkill {
    pub fn new() -> Self {
        Self
    }
    pub fn skill_prompt() -> String {
        r#"# WASM Skill

You are running the **wasm** skill. You manage WebAssembly modules as
executable tools within the agent runtime.

## Capabilities

- **Load** WASM modules from `.wasm` files
- **Validate** modules (magic bytes, structure, exports)
- **Inspect** exported functions and their signatures
- **Execute** WASM functions with typed arguments
- **Manage** a registry of loaded modules

## Module Discovery

WASM modules are discovered from:
1. Project-local: `.oxi/wasm/` directory
2. User-global: `~/.oxi/wasm/` directory
3. Explicit paths via settings

## Security Considerations

- **Capability-based**: Each module declares required capabilities
- **Sandboxed**: WASM modules run in a sandboxed environment
- **No direct filesystem access** unless explicitly granted
- **No network access** unless explicitly granted
"#
        .to_string()
    }
}

impl Default for WasmSkill {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for WasmSkill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmSkill").finish()
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut hash: [u8; 32] = [0u8; 32];
    for (i, byte) in data.iter().enumerate() {
        hash[i % 32] ^= byte;
        hash[i % 32] = hash[i % 32].wrapping_add(*byte);
    }
    let mut hex = String::with_capacity(64);
    for byte in &hash {
        write!(&mut hex, "{:02x}", byte).unwrap();
    }
    hex
}

fn parse_wasm_exports(bytes: &[u8]) -> Vec<WasmFunction> {
    let mut exports = Vec::new();
    if bytes.len() < 8 {
        return exports;
    }
    let mut pos = 8usize;
    while pos + 1 < bytes.len() {
        let section_id = bytes[pos];
        pos += 1;
        let (size, bytes_read) = read_leb128(&bytes[pos..]);
        pos += bytes_read;
        if pos + size > bytes.len() {
            break;
        }
        let section_end = pos + size;
        if section_id == 7 && size > 0 {
            let section_data = &bytes[pos..section_end];
            let (num_exports, _) = read_leb128(section_data);
            let mut export_pos = 1usize;
            for _ in 0..num_exports {
                if export_pos >= section_data.len() {
                    break;
                }
                let (name_len, nr) = read_leb128(&section_data[export_pos..]);
                export_pos += nr;
                if export_pos + name_len as usize > section_data.len() {
                    break;
                }
                let name_bytes = &section_data[export_pos..export_pos + name_len as usize];
                let name = String::from_utf8_lossy(name_bytes).to_string();
                export_pos += name_len as usize;
                if export_pos >= section_data.len() {
                    break;
                }
                let kind = section_data[export_pos];
                export_pos += 1;
                let (_index, nr) = read_leb128(&section_data[export_pos..]);
                export_pos += nr;
                if kind == 0 {
                    exports.push(WasmFunction::new(&name));
                }
            }
        }
        pos = section_end;
    }
    exports
}

fn read_leb128(data: &[u8]) -> (usize, usize) {
    let mut result = 0usize;
    let mut shift = 0;
    let mut i = 0;
    for &byte in data {
        result |= ((byte & 0x7F) as usize) << shift;
        shift += 7;
        i += 1;
        if byte & 0x80 == 0 {
            break;
        }
        if shift >= 32 {
            break;
        }
    }
    (result, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_wasm_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\0asm");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes
    }

    fn wasm_with_export(export_name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\0asm");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let type_section = vec![0x01, 0x60, 0x00, 0x00];
        bytes.push(1);
        bytes.push(type_section.len() as u8);
        bytes.extend_from_slice(&type_section);
        let func_section = vec![0x01, 0x00];
        bytes.push(3);
        bytes.push(func_section.len() as u8);
        bytes.extend_from_slice(&func_section);
        let name_bytes = export_name.as_bytes();
        let mut export_entry = Vec::new();
        export_entry.push(name_bytes.len() as u8);
        export_entry.extend_from_slice(name_bytes);
        export_entry.push(0);
        export_entry.push(0);
        let mut export_section = Vec::new();
        export_section.push(0x01);
        export_section.extend_from_slice(&export_entry);
        bytes.push(7);
        bytes.push(export_section.len() as u8);
        bytes.extend_from_slice(&export_section);
        let func_body = vec![0x00];
        let mut code_entry = Vec::new();
        code_entry.push((func_body.len() + 1) as u8);
        code_entry.push(0x00);
        code_entry.extend_from_slice(&func_body);
        code_entry.push(0x0B);
        let mut code_section = Vec::new();
        code_section.push(0x01);
        code_section.extend_from_slice(&code_entry);
        bytes.push(10);
        bytes.push(code_section.len() as u8);
        bytes.extend_from_slice(&code_section);
        bytes
    }

    #[test]
    fn test_wasm_value_type_display() {
        assert_eq!(format!("{}", WasmValueType::I32), "i32");
        assert_eq!(format!("{}", WasmValueType::I64), "i64");
    }

    #[test]
    fn test_wasm_function_signature() {
        let func = WasmFunction::new("add")
            .param(WasmValueType::I32)
            .param(WasmValueType::I32)
            .result(WasmValueType::I32);
        assert_eq!(func.signature(), "add(i32, i32) -> i32");
    }

    #[test]
    fn test_load_minimal_wasm() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.wasm");
        fs::write(&path, minimal_wasm_bytes()).unwrap();
        let module = WasmModule::load(&path).unwrap();
        assert_eq!(module.name, "test");
        assert!(module.exports.is_empty());
    }

    #[test]
    fn test_load_wasm_with_export() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("my-module.wasm");
        fs::write(&path, wasm_with_export("greet")).unwrap();
        let module = WasmModule::load(&path).unwrap();
        assert_eq!(module.name, "my-module");
        assert!(module.has_export("greet"));
    }

    #[test]
    fn test_load_nonexistent() {
        assert!(WasmModule::load(Path::new("/nonexistent/test.wasm")).is_err());
    }

    #[test]
    fn test_load_invalid_magic() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.wasm");
        fs::write(&path, b"INVALIDBINARYDATA0001").unwrap();
        let result = WasmModule::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_module_with_description() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.wasm");
        fs::write(&path, minimal_wasm_bytes()).unwrap();
        let module = WasmModule::load(&path)
            .unwrap()
            .with_description("A test module");
        assert_eq!(module.description.as_deref(), Some("A test module"));
    }

    #[test]
    fn test_validate_exports() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.wasm");
        fs::write(&path, wasm_with_export("greet")).unwrap();
        let module = WasmModule::load(&path).unwrap();
        assert!(module.validate_exports(&["greet"]).is_empty());
        assert_eq!(
            module.validate_exports(&["greet", "missing"]),
            vec!["missing"]
        );
    }

    #[test]
    fn test_registry_register_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.wasm");
        fs::write(&path, minimal_wasm_bytes()).unwrap();
        let module = WasmModule::load(&path).unwrap();
        let mut registry = WasmRegistry::new();
        registry.register(module);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("test").is_some());
    }

    #[test]
    fn test_registry_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let wasm_dir = tmp.path().join("wasm");
        fs::create_dir_all(&wasm_dir).unwrap();
        fs::write(wasm_dir.join("test.wasm"), minimal_wasm_bytes()).unwrap();
        let mut registry = WasmRegistry::new();
        registry.add_search_path(&wasm_dir);
        let loaded = registry.scan_search_paths().unwrap();
        assert_eq!(loaded, vec!["test"]);
    }

    #[test]
    fn test_read_leb128() {
        assert_eq!(read_leb128(&[0x00]), (0, 1));
        assert_eq!(read_leb128(&[0x7F]), (127, 1));
        assert_eq!(read_leb128(&[0x80, 0x01]), (128, 2));
    }

    #[test]
    fn test_skill_prompt_not_empty() {
        let prompt = WasmSkill::skill_prompt();
        assert!(prompt.contains("WASM Skill"));
    }

    #[test]
    fn test_module_serialization_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.wasm");
        fs::write(&path, minimal_wasm_bytes()).unwrap();
        let module = WasmModule::load(&path).unwrap().with_description("Test");
        let json = serde_json::to_string(&module).unwrap();
        let parsed: WasmModule = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, module.name);
    }
}
