//! `memory://` URL protocol handler.
//!
//! Resolves the documented artifact paths from the autonomous
//! memory pipeline (omp `memory-protocol.ts` port):
//!
//! - `memory://root` → short listing of `MEMORY.md`,
//!   `memory_summary.md`, `learned.md`, and any `skills/<name>/`
//!   directories.
//! - `memory://root/MEMORY.md`,
//!   `memory://root/memory_summary.md`,
//!   `memory://root/learned.md` → the corresponding artifact.
//! - `memory://root/skills/<name>/SKILL.md` → the skill playbook.
//!
//! The router is resolved through the wired `Oxi`
//! `InternalUrlRouter` port. When the local pipeline has not run
//! yet, every path resolves to an empty listing.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use oxi_sdk::SdkError;
use oxi_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl};

/// Resolve a `memory://` URL to plain markdown text.
///
/// `memory_root` is the project's `<home>/memory/<encoded-cwd>/`
/// directory (see `memory_summary::memory_root`).
pub fn resolve_memory_url(url: &str, memory_root: &Path) -> Option<String> {
    let suffix = url.strip_prefix("memory://")?;
    let suffix = suffix.trim_start_matches("root/");
    let suffix = suffix.trim_start_matches("root");
    let suffix = suffix.trim_start_matches('/');

    if suffix.is_empty() {
        return Some(list_root(memory_root));
    }

    let candidate = memory_root.join(suffix);
    if !is_within(memory_root, &candidate) {
        return None;
    }
    std::fs::read_to_string(&candidate).ok()
}

fn is_within(root: &Path, candidate: &Path) -> bool {
    let Ok(r) = root.canonicalize() else {
        return false;
    };
    let Ok(c) = candidate.canonicalize() else {
        return false;
    };
    c.starts_with(r)
}

fn list_root(memory_root: &Path) -> String {
    let mut out =
        String::from("# Memory root\n\nListing of artifacts at the project memory root.\n");
    let entries = std::fs::read_dir(memory_root).ok();
    let has_files = entries
        .map(|rd| {
            rd.flatten().any(|e| {
                let p = e.path();
                p.is_file() || p.is_dir()
            })
        })
        .unwrap_or(false);
    if !has_files {
        out.push_str("(empty — pipeline has not run yet)\n");
        return out;
    }
    out.push_str("- `memory://root/MEMORY.md`\n");
    out.push_str("- `memory://root/memory_summary.md`\n");
    out.push_str("- `memory://root/learned.md`\n");
    out.push_str("- `memory://root/skills/<name>/SKILL.md`\n");
    out
}

/// Protocol-handler wrapper that resolves `memory://…` URLs against
/// a single memory root registered at construction time.
pub struct MemoryProtocolHandler {
    memory_root: PathBuf,
}

impl MemoryProtocolHandler {
    pub fn new(memory_root: PathBuf) -> Self {
        Self { memory_root }
    }
}

#[async_trait]
impl ProtocolHandler for MemoryProtocolHandler {
    fn scheme(&self) -> &str {
        "memory"
    }

    async fn resolve(
        &self,
        url: &str,
        _selector: Option<&str>,
        _ctx: &ResolveContext,
    ) -> Result<ResolvedUrl, SdkError> {
        let content = resolve_memory_url(url, &self.memory_root)
            .ok_or_else(|| SdkError::PortNotConfigured { port: "memory" })?;
        let size = content.len();
        Ok(ResolvedUrl {
            url: url.to_string(),
            content,
            content_type: "text/markdown".to_string(),
            size: Some(size),
            source_path: None,
            notes: vec![],
            immutable: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_root_returns_listing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("MEMORY.md"), "hi").unwrap();
        let out = resolve_memory_url("memory://root", dir.path()).unwrap();
        assert!(out.contains("Memory root"));
    }

    #[test]
    fn resolve_specific_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("MEMORY.md");
        std::fs::write(&p, "hello").unwrap();
        let out = resolve_memory_url("memory://root/MEMORY.md", dir.path()).unwrap();
        assert_eq!(out, "hello");
    }

    #[test]
    fn traversal_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_memory_url("memory://root/../../etc/passwd", dir.path()).is_none());
    }
}
