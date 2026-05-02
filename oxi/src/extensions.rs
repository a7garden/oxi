//! Extension system for oxi
//!
//! Extensions allow custom tools, commands, and event hooks to be loaded
//! dynamically at runtime. Extensions can be loaded from shared libraries
//! (.so/.dll/.dylib) via the `-e`/`--extension` CLI flag.

use anyhow::{bail, Context, Result};
use libloading::{Library, Symbol};
use oxi_agent::{AgentEvent, AgentTool};
use std::ffi::OsStr;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// A simple command definition for the CLI.
#[derive(Debug, Clone)]
pub struct Command {
    /// Slash-command name (e.g. "deploy")
    pub name: String,
    /// Short description shown in /help
    pub description: String,
    /// Usage string (e.g. "/deploy <target>")
    pub usage: String,
}

impl Command {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        usage: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            usage: usage.into(),
        }
    }
}

/// Core trait that every oxi extension must implement.
///
/// Extensions can register custom tools, custom slash-commands, and hook
/// into the agent event stream.
pub trait Extension: Send + Sync {
    /// Unique name of the extension.
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// Return custom tools this extension contributes.
    fn register_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![]
    }

    /// Return custom slash-commands this extension contributes.
    fn register_commands(&self) -> Vec<Command> {
        vec![]
    }

    /// Called when the agent emits an event.
    fn on_event(&self, _event: &AgentEvent) {}
}

// ── ExtensionRegistry ──────────────────────────────────────────────────

/// Manages a collection of loaded extensions.
pub struct ExtensionRegistry {
    extensions: Vec<Arc<dyn Extension>>,
    /// Keep the libraries alive so the vtables stay valid.
    #[allow(dead_code)]
    libraries: Vec<Library>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
            libraries: Vec::new(),
        }
    }

    /// Register an extension (in-memory).
    pub fn register(&mut self, ext: Arc<dyn Extension>) {
        tracing::info!(name = ext.name(), "extension registered");
        self.extensions.push(ext);
    }

    /// Collect all tools from every registered extension.
    pub fn all_tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.extensions
            .iter()
            .flat_map(|e| e.register_tools())
            .collect()
    }

    /// Collect all commands from every registered extension.
    pub fn all_commands(&self) -> Vec<Command> {
        self.extensions
            .iter()
            .flat_map(|e| e.register_commands())
            .collect()
    }

    /// Broadcast an event to every extension.
    pub fn emit_event(&self, event: &AgentEvent) {
        for ext in &self.extensions {
            ext.on_event(event);
        }
    }

    /// Iterate over registered extensions.
    pub fn extensions(&self) -> impl Iterator<Item = &Arc<dyn Extension>> {
        self.extensions.iter()
    }

    /// Number of registered extensions.
    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    /// Whether any extensions are registered.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }
}

impl fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRegistry")
            .field("count", &self.extensions.len())
            .finish()
    }
}

// ── Dynamic loading ────────────────────────────────────────────────────

/// Expected symbol name inside a shared-library extension.
const ENTRY_SYMBOL: &[u8] = b"oxi_extension_create\0";

/// Function signature that a shared library must export.
///
/// The library must expose:
///
/// ```c,ignore
/// extern "C" fn oxi_extension_create() -> *mut dyn Extension
/// ```
type CreateFn = unsafe fn() -> *mut dyn Extension;

/// Load an extension from a shared library (.so / .dll / .dylib).
///
/// The library **must** export an `oxi_extension_create` entry-point that
/// returns a heap-allocated trait object.
pub fn load_extension(path: &Path) -> Result<Arc<dyn Extension>> {
    let extension = load_extension_inner(path)?;
    Ok(extension)
}

fn load_extension_inner(path: &Path) -> Result<Arc<dyn Extension>> {
    // Validate file extension
    let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");

    let valid = matches!(ext, "so" | "dylib" | "dll");
    if !valid {
        bail!(
            "Unsupported extension file format: .{}. Expected .so, .dylib, or .dll",
            ext
        );
    }

    if !path.exists() {
        bail!("Extension file not found: {}", path.display());
    }

    // Safety: loading a shared library is inherently unsafe. We trust the
    // user-provided library to be well-behaved.
    let library = unsafe {
        Library::new(path).with_context(|| format!("Failed to load library: {}", path.display()))?
    };

    let create: Symbol<CreateFn> = unsafe {
        library.get(ENTRY_SYMBOL).with_context(|| {
            format!(
                "Symbol `oxi_extension_create` not found in {}",
                path.display()
            )
        })?
    };

    let raw_ptr = unsafe { create() };
    if raw_ptr.is_null() {
        bail!("oxi_extension_create returned null in {}", path.display());
    }

    // Wrap the raw pointer in an Arc directly via Box
    let boxed: Box<dyn Extension> = unsafe { Box::from_raw(raw_ptr) };
    Ok(Arc::from(boxed))
}

/// Load multiple extensions from file paths, collecting errors.
pub fn load_extensions(paths: &[&Path]) -> (Vec<Arc<dyn Extension>>, Vec<anyhow::Error>) {
    let mut loaded = Vec::with_capacity(paths.len());
    let mut errors = Vec::new();

    for &path in paths {
        match load_extension(path) {
            Ok(ext) => loaded.push(ext),
            Err(e) => {
                errors.push(e.context(format!("Failed to load extension: {}", path.display())))
            }
        }
    }

    (loaded, errors)
}

// ── Built-in "noop" extension for testing ──────────────────────────────

/// A minimal extension that does nothing — useful as a template and for tests.
pub struct NoopExtension;

impl Extension for NoopExtension {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "Built-in no-op extension"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_and_collect() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));

        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
        assert!(reg.all_tools().is_empty());
        assert!(reg.all_commands().is_empty());
    }

    #[test]
    fn test_emit_event_does_not_panic() {
        let mut reg = ExtensionRegistry::new();
        reg.register(Arc::new(NoopExtension));
        reg.emit_event(&AgentEvent::Thinking);
    }

    #[test]
    fn test_command_new() {
        let cmd = Command::new("deploy", "Deploy the project", "/deploy <target>");
        assert_eq!(cmd.name, "deploy");
        assert_eq!(cmd.description, "Deploy the project");
        assert_eq!(cmd.usage, "/deploy <target>");
    }

    #[test]
    fn test_load_extension_missing_file() {
        let result = load_extension(Path::new("/nonexistent/extension.so"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_extension_wrong_extension() {
        let result = load_extension(Path::new("something.txt"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Unsupported extension file format"));
    }

    #[test]
    fn test_load_extensions_collects_errors() {
        let paths: Vec<&Path> = vec![Path::new("/nonexistent1.so"), Path::new("/nonexistent2.so")];
        let (loaded, errors) = load_extensions(&paths);
        assert!(loaded.is_empty());
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn test_registry_debug() {
        let reg = ExtensionRegistry::new();
        let debug_str = format!("{:?}", reg);
        assert!(debug_str.contains("count"));
    }
}
