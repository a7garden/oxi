#![allow(deprecated, dead_code, unused_imports, unused_variables, unused_mut, clippy::all)]

//! Product-crate API shims for the vendored grok TUI presentation layer.
//! Provides the minimal types and functions that grok's render crates
//! import from xai-grok-config / xai-grok-shared / xai-grok-workspace.
//! Not full upstream copies — just the surface needed by the render layer.
pub mod clipboard;
pub mod config;
pub mod session;
pub mod stderr;
pub mod telemetry;
pub mod tools;
pub mod ui_config;
pub mod workspace;
