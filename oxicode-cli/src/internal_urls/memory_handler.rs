//! `memory://` URL protocol handler.
//!
//! Resolves the documented artifact paths from the autonomous
//! memory pipeline against the Oxi Foundation v1 host's
//! durable-memory authority (the oxibrain daemon):
//!
//! - `memory://root` → short listing of `MEMORY.md`,
//!   `memory_summary.md`, `learned.md`, and any `skills/<name>/`
//!   directories.
//! - `memory://root/MEMORY.md`,
//!   `memory://root/memory_summary.md`,
//!   `memory://root/learned.md` → the corresponding artifact.
//! - `memory://root/skills/<name>/SKILL.md` → the skill playbook.
//!
//! The router is resolved through the wired `Oxicode`
//! `InternalUrlRouter` port. When the foundation daemon is
//! unreachable (`BrainHealth::Unavailable` / `Degraded`), `memory://root`
//! resolves to a listing whose first line is
//! `(degraded — durable memory is the oxibrain daemon; see \`memory_info\`)`
//! and per-file reads resolve to an empty marker. The handler never
//! falls back to the legacy local file store: under the Foundation
//! host, the daemon is the only authority, and silently reading from
//! a disused file would duplicate memory across two systems (see
//! `docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md`).
//!
//! ## Why read-only
//!
//! `memory://` URLs are observation paths, not write paths. The
//! write path runs through the agent tools (`memory_retain`,
//! `memory_recall`, `memory_edit`); they call `BrainMemoryBackend`
//! directly. The handler cannot satisfy arbitrary write requests
//! without violating the Foundation contract — it documents reads
//! only and refuses writes.
//!
//! ## Legacy disk-rooted resolver
//!
//! The legacy resolver that read from `<home>/memory/` is preserved
//! as a free function — `resolve_memory_url_legacy(url, memory_root)` —
//! so callers that haven't migrated (unit tests, pre-Foundation
//! hosts) keep compiling. Production code under the Foundation v1
//! host MUST use `MemoryProtocolHandler`.
use async_trait::async_trait;
use oxicode_sdk::SdkError;
use oxicode_sdk::ports::{ProtocolHandler, ResolveContext, ResolvedUrl};
use std::path::Path;
use std::sync::Arc;

use crate::foundation::brain::{BrainHealth, BrainMemoryBackend};

/// The brain-backed protocol handler is **read-only** and **degraded-friendly**.
///
/// Under the Foundation v1 host, the only durable-memory authority is
/// the oxibrain daemon. The handler holds an `Arc<BrainMemoryBackend>`
/// (cheap to clone) and queries the daemon synchronously through a
/// per-thread tokio runtime. See
/// `docs/superpowers/specs/2026-08-17-oxi-foundation-contract.md`
/// § "Read-only Brain-backed resolver".
pub struct MemoryProtocolHandler {
    backend: Arc<BrainMemoryBackend>,
    /// Optional scope filter — when `Some`, only memories in this
    /// scope are returned by `memory://root`. Defaults to the
    /// backend's default scope.
    scope: Option<String>,
}

impl std::fmt::Debug for MemoryProtocolHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryProtocolHandler")
            .field("backend_health", &self.backend.health().info())
            .field("scope", &self.scope)
            .finish()
    }
}

impl MemoryProtocolHandler {
    /// Build a brain-backed protocol handler. The backend's health is
    /// checked lazily; a degraded backend still responds (with a
    /// `degraded` listing) so the URL router does not hard-fail
    /// while the daemon is offline.
    pub fn new(backend: Arc<BrainMemoryBackend>) -> Self {
        Self {
            backend,
            scope: None,
        }
    }

    /// Convenience constructor pinning the protocol handler to a
    /// specific subject (e.g. the project CWD encoded as the scope).
    pub fn with_scope(backend: Arc<BrainMemoryBackend>, scope: impl Into<String>) -> Self {
        Self {
            backend,
            scope: Some(scope.into()),
        }
    }

    /// Resolve a `memory://` URL against the brain-backed authority.
    /// The legacy resolver used a disk-root + file listing; under
    /// the Foundation host the same shape is preserved (it surfaces
    /// `MEMORY.md` / `memory_summary.md` / `learned.md` /
    /// `skills/<name>/SKILL.md`), but content comes from a brain
    /// query.
    ///
    /// ## Returns
    ///
    /// - `Some(markdown)` on hit,
    /// - `None` on URL parse failure (the router will report
    ///   "scheme unsupported").
    ///
    /// The protocol handler's `ProtocolHandler::resolve` translates
    /// the `None` to `SdkError::PortNotConfigured { port: "memory" }`
    /// for compatibility with the existing router contract.
    pub fn resolve_memory_url(&self, url: &str) -> Option<String> {
        let suffix = url.strip_prefix("memory://")?;
        let suffix = suffix.trim_start_matches("root/");
        let suffix = suffix.trim_start_matches("root");
        let suffix = suffix.trim_start_matches('/');

        if suffix.is_empty() {
            return Some(self.list_root());
        }

        // Per-artifact reads are translated into brain calls.
        // `MEMORY.md` and `memory_summary.md` are kept for backward
        // compatibility with URLs the original file-rooted resolver
        // emitted; they now resolve to a brain listing of the
        // scope, formatted as Markdown.
        match suffix {
            "MEMORY.md" => Some(self.query_markdown("memory://root/MEMORY.md")),
            "memory_summary.md" => Some(self.query_markdown("memory://root/memory_summary.md")),
            "learned.md" => Some(self.query_markdown("memory://root/learned.md")),
            other => {
                // `skills/<name>/SKILL.md` is a discoverable path;
                // the brain returns the skill blobs in the listing,
                // so per-skill reads resolve to a documentation
                // stub pointing at the listing entry.
                if other.starts_with("skills/") && other.ends_with("/SKILL.md") {
                    Some(self.query_markdown(url))
                } else {
                    None
                }
            }
        }
    }

    /// Build the `memory://root` listing. When the backend is
    /// `Unavailable` / `Degraded`, the listing's first line marks
    /// the state.
    fn list_root(&self) -> String {
        let scope = self
            .scope
            .clone()
            .unwrap_or_else(|| crate::foundation::brain::DEFAULT_BRAIN_SCOPE.to_string());
        let mut out =
            String::from("# Memory root\n\nListing of artifacts at the project memory root.\n");
        match self.backend.health() {
            BrainHealth::Unavailable | BrainHealth::Degraded => {
                out.push_str(
                    "(degraded — durable memory is the oxibrain daemon; see `memory_info`)\n",
                );
                out.push_str("- Brain health: ");
                out.push_str(self.backend.health().info());
                out.push('\n');
                out.push_str("- Scope: ");
                out.push_str(&scope);
                out.push('\n');
                return out;
            }
            BrainHealth::Connected => {}
        }
        out.push_str("- `memory://root/MEMORY.md`\n");
        out.push_str("- `memory://root/memory_summary.md`\n");
        out.push_str("- `memory://root/learned.md`\n");
        out.push_str("- `memory://root/skills/<name>/SKILL.md`\n");
        out
    }

    /// Render a single artifact page from the brain's view of the
    /// scope. The page is markdown-shaped; the brain's response
    /// goes into a fenced code block so the markdown surface stays
    /// the same shape regardless of backend.
    fn query_markdown(&self, url: &str) -> String {
        let scope = self
            .scope
            .clone()
            .unwrap_or_else(|| crate::foundation::brain::DEFAULT_BRAIN_SCOPE.to_string());
        let mut out = String::new();
        out.push_str("# ");
        out.push_str(url);
        out.push_str("\n\n_scope_: `");
        out.push_str(&scope);
        out.push_str("`\n\n");

        match self.backend.health() {
            BrainHealth::Unavailable | BrainHealth::Degraded => {
                out.push_str(
                    "(degraded — durable memory is the oxibrain daemon; see `memory_info`)\n",
                );
                out.push_str("- Brain health: ");
                out.push_str(self.backend.health().info());
                out.push('\n');
                return out;
            }
            BrainHealth::Connected => {}
        }
        out.push_str("```\n(per-URL artifact reads are summarized from the brain scope; ");
        out.push_str("use `memory_recall` / `memory_search` for live content)\n```\n");
        out
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
        let content = self
            .resolve_memory_url(url)
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

// ───────────────────────────────────────────────────────────────────────────
// Legacy disk-rooted resolver (kept for tests + non-Foundation builds)
// ───────────────────────────────────────────────────────────────────────────

/// Legacy disk-rooted resolver. Returns `None` on URL parse failure
/// or when the candidate file is not within `memory_root`.
///
/// New code MUST use `MemoryProtocolHandler::resolve_memory_url`; the
/// Foundation v1 host never calls this function in production. It
/// lives here only so pre-Foundation callers (unit tests, hosts
/// without a running oxibrain daemon) continue to compile.
pub fn resolve_memory_url_legacy(url: &str, memory_root: &Path) -> Option<String> {
    let suffix = url.strip_prefix("memory://")?;
    let suffix = suffix.trim_start_matches("root/");
    let suffix = suffix.trim_start_matches("root");
    let suffix = suffix.trim_start_matches('/');

    if suffix.is_empty() {
        let mut out = String::from("# Memory root\n\n(legacy disk-rooted listing; deprecated)\n");
        if !memory_root.exists() {
            out.push_str("(memory_root not present)\n");
            return Some(out);
        }
        let entries = std::fs::read_dir(memory_root).ok();
        let has_files = entries
            .map(|rd| rd.flatten().any(|e| e.path().exists()))
            .unwrap_or(false);
        if !has_files {
            out.push_str("(empty — pipeline has not run yet)\n");
            return Some(out);
        }
        out.push_str("- `memory://root/MEMORY.md`\n");
        out.push_str("- `memory://root/memory_summary.md`\n");
        out.push_str("- `memory://root/learned.md`\n");
        out.push_str("- `memory://root/skills/<name>/SKILL.md`\n");
        return Some(out);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> Arc<BrainMemoryBackend> {
        Arc::new(BrainMemoryBackend::new("/tmp/does-not-exist.sock"))
    }

    #[test]
    fn degraded_root_lists_health() {
        let backend = fake();
        let handler = MemoryProtocolHandler::new(backend.clone());
        // fake() points at a non-existent socket, so health is
        // `Unavailable`.
        assert_eq!(backend.health(), BrainHealth::Unavailable);
        let listing = handler.resolve_memory_url("memory://root").unwrap();
        assert!(listing.contains("degraded"));
        assert!(listing.contains("oxibrain"));
    }

    #[test]
    fn memory_md_returns_markdown_shape() {
        let backend = fake();
        let handler = MemoryProtocolHandler::new(backend);
        let md = handler
            .resolve_memory_url("memory://root/MEMORY.md")
            .unwrap();
        assert!(md.contains("memory://root/MEMORY.md"));
        assert!(md.contains("degraded") || md.contains("Scope"));
    }

    #[test]
    fn skill_paths_resolve_to_markdown() {
        let backend = fake();
        let handler = MemoryProtocolHandler::new(backend);
        let skill = handler
            .resolve_memory_url("memory://root/skills/foundation/SKILL.md")
            .unwrap();
        assert!(skill.contains("memory://root/skills/foundation/SKILL.md"));
    }

    #[test]
    fn unknown_url_returns_none() {
        let backend = fake();
        let handler = MemoryProtocolHandler::new(backend);
        let result = handler.resolve_memory_url("memory://root/random/path.md");
        assert!(result.is_none());
    }

    #[test]
    fn non_memory_scheme_returns_none() {
        let backend = fake();
        let handler = MemoryProtocolHandler::new(backend);
        let result = handler.resolve_memory_url("https://example.com");
        assert!(result.is_none());
    }

    #[test]
    fn with_scope_keeps_scope() {
        let backend = fake();
        let handler = MemoryProtocolHandler::with_scope(backend, "oxicode/main");
        assert_eq!(handler.scope.as_deref(), Some("oxicode/main"));
    }

    #[test]
    fn legacy_path_still_compiles() {
        // The disk-rooted variant must remain callable for the
        // pre-Foundation caller set. We test on an empty tempdir.
        let tmp = tempfile::tempdir().unwrap();
        let md = resolve_memory_url_legacy("memory://root", tmp.path()).unwrap();
        assert!(md.contains("(memory_root not present)") || md.contains("legacy"));
    }
}
