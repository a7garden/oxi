//! Storage and resources — packages, resources (skills/themes/prompts), export
pub mod packages;
pub(crate) mod resource_loader;
pub(crate) mod resource_loader_compat;
pub(crate) mod export;

// Re-exports for convenience
pub use resource_loader::{ResourceLoader, Skill, Theme, Prompt};
pub use export::{ExportMeta, HtmlExportOptions};
