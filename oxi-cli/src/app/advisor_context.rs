//! Advisor system-prompt assembly — injects project context files
//! (AGENTS.md / CLAUDE.md) and WATCHDOG.md attention files so the read-only
//! reviewer can hold the primary agent to the user's standing project
//! instructions. Ported from omp `watchdog.ts`.
//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), MIT licensed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::storage::resource_loader::ResourceLoader;

/// Assemble the advisor's full system prompt:
/// the base advisor prompt + a `<project-context>` block (AGENTS.md etc.) +
/// any WATCHDOG.md `<attention>` blocks discovered walking up from `cwd`.
///
/// Returns just the base prompt when there are no context/watchdog files
/// (the common case outside a project).
#[must_use]
pub fn assemble_advisor_system_prompt(cwd: &str) -> String {
    let mut out = oxi_agent::ADVISOR_SYSTEM_PROMPT.to_string();

    // ── Project context files (AGENTS.md, CLAUDE.md) ──
    // omp `formatAdvisorContextPrompt` — gives the read-only reviewer the
    // user's standing project instructions so it can flag drift instead of
    // advising against conventions it cannot otherwise see.
    let context_files = ResourceLoader::new().load_project_context_files(Path::new(cwd));
    if let Ok(files) = &context_files
        && !files.is_empty()
    {
        let mut block = String::from(
            "\n\n<project-context>\nThese context files carry the user's standing instructions \
             for this project (AGENTS.md and the like). The driving agent is bound by them. \
             Hold the agent to them and flag drift the moment it starts; never advise against \
             what these files mandate.",
        );
        for f in files {
            let display_path = f
                .path
                .strip_prefix(cwd)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| f.name.clone());
            block.push_str(&format!("\n<file path=\"{}\">\n{}\n</file>", display_path, f.content));
        }
        block.push_str("\n</project-context>");
        out.push_str(&block);
    }

    // ── WATCHDOG.md attention files ──
    // omp `discoverWatchdogFiles` — user-level (~/.oxi/WATCHDOG.md) first,
    // then project levels walking cwd → home (ancestor → leaf).
    let watchdogs = discover_watchdog_files(Path::new(cwd));
    for (display_path, content) in watchdogs {
        out.push_str(&format!(
            "\n\nEspecially pay attention to:\n<attention source=\"{}\">\n{}\n</attention>",
            display_path, content
        ));
    }

    out
}

/// Discover WATCHDOG.md files: the user-level file (`~/.oxi/WATCHDOG.md`) plus
/// any walking up from `cwd` toward the home directory. Returns
/// `(display_path, content)` pairs, user-level first then ancestor → leaf.
/// omp `discoverWatchdogFiles`.
fn discover_watchdog_files(cwd: &Path) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    // 1. User level: ~/.oxi/WATCHDOG.md
    if let Some(home) = dirs::home_dir() {
        let user_path = home.join(".oxi").join("WATCHDOG.md");
        if let Some(content) = read_watchdog(&user_path) {
            seen.insert(user_path.clone());
            let display = user_path.display().to_string();
            items.push((display, content));
        }

        // 2. Project levels: walk cwd → home, collecting WATCHDOG.md
        let mut current = Some(cwd);
        let mut ascending: Vec<PathBuf> = Vec::new();
        while let Some(dir) = current
            && dir != home
        {
            ascending.push(dir.to_path_buf());
            current = dir.parent();
        }
        // ancestor → leaf (outermost first)
        for dir in ascending.into_iter().rev() {
            let candidate = dir.join("WATCHDOG.md");
            if seen.contains(&candidate) {
                continue;
            }
            if let Some(content) = read_watchdog(&candidate) {
                let display = candidate
                    .strip_prefix(cwd)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| candidate.display().to_string());
                seen.insert(candidate);
                items.push((display, content));
            }
        }
    }

    items
}

/// Read a WATCHDOG.md file if it exists and is non-empty.
fn read_watchdog(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(content)
    }
}

// ── Transcript recorder ────────────────────────────────────────────────────

/// Reserved transcript stem for advisor session files. omp
/// `ADVISOR_TRANSCRIPT_STEM`. Chosen so it cannot collide with a task
/// subagent's `<id>.jsonl`.
pub const ADVISOR_TRANSCRIPT_FILENAME: &str = "__advisor.jsonl";

/// Append-only persister for an advisor agent's turns to
/// `<session_dir>/__advisor.jsonl`, for stats attribution / observability.
/// omp `AdvisorTranscriptRecorder`.
///
/// Fed by [`AgentAdvisor::with_post_prompt_hook`](oxi_agent::AgentAdvisor) —
/// after each successful advisor prompt, the host records the advisor's new
/// messages (the delta since the last recorded count).
pub struct AdvisorTranscriptRecorder {
    path: Option<PathBuf>,
    cursor: Arc<std::sync::atomic::AtomicUsize>,
}

impl AdvisorTranscriptRecorder {
    /// Construct from the primary session file path; the advisor transcript
    /// lives in the same directory. `None` (no-op recorder) when there is no
    /// session file (e.g. in-memory test sessions).
    #[must_use]
    pub fn new(session_file: Option<String>) -> Self {
        let path = session_file
            .as_ref()
            .and_then(|f| Path::new(f).parent().map(|dir| dir.join(ADVISOR_TRANSCRIPT_FILENAME)));
        Self {
            path,
            cursor: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Record the advisor's messages added since the last call. Appends one
    /// JSON line carrying the new messages' text. Best-effort: I/O errors are
    /// logged and swallowed (the advisor must never fail a turn because the
    /// transcript couldn't be written).
    pub fn record(&self, agent: &oxi_agent::Agent) {
        use std::io::Write;
        use std::sync::atomic::Ordering;
        let Some(path) = &self.path else {
            return;
        };
        let messages = agent.state().messages;
        let prev = self.cursor.load(Ordering::SeqCst);
        if messages.len() <= prev {
            return;
        }
        let texts: Vec<String> = messages[prev..]
            .iter()
            .map(|m| m.text_content().unwrap_or_default())
            .collect();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let line = serde_json::json!({
            "ts": now_ms,
            "messages": texts,
        });
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(mut f) => {
                if let Err(e) = writeln!(f, "{line}") {
                    tracing::warn!("advisor transcript write failed: {e}");
                }
            }
            Err(e) => tracing::warn!("advisor transcript open failed: {e}"),
        }
        self.cursor.store(messages.len(), Ordering::SeqCst);
    }

    /// Build the post-prompt hook for [`AgentAdvisor::with_post_prompt_hook`].
    #[must_use]
    pub fn hook(self: &Arc<Self>) -> Arc<dyn Fn(&oxi_agent::Agent) + Send + Sync> {
        let this = Arc::clone(self);
        Arc::new(move |agent| this.record(agent))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn base_prompt_when_no_context() {
        // In a tmpdir with no AGENTS.md / WATCHDOG.md, only the base prompt.
        let tmp = tempfile::tempdir().unwrap();
        let prompt = assemble_advisor_system_prompt(tmp.path().to_str().unwrap());
        assert!(prompt.contains("shadow the main agent"));
        assert!(!prompt.contains("<project-context>"));
        assert!(!prompt.contains("<attention"));
    }

    #[test]
    fn injects_agents_md_context() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# Rules\nNever commit to main").unwrap();
        let cwd = tmp.path().to_str().unwrap();
        let prompt = assemble_advisor_system_prompt(cwd);
        assert!(prompt.contains("<project-context>"), "must wrap context files");
        assert!(prompt.contains("Never commit to main"));
        assert!(prompt.contains("AGENTS.md"));
    }

    #[test]
    fn injects_watchdog_attention() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("WATCHDOG.md"), "Block any change to the lockfile").unwrap();
        let prompt = assemble_advisor_system_prompt(tmp.path().to_str().unwrap());
        assert!(prompt.contains("<attention"));
        assert!(prompt.contains("Block any change to the lockfile"));
    }

    #[test]
    fn empty_watchdog_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("WATCHDOG.md"), "   \n  ").unwrap();
        let prompt = assemble_advisor_system_prompt(tmp.path().to_str().unwrap());
        assert!(!prompt.contains("<attention"));
    }
}
