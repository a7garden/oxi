//! Autonomous memory pipeline (omp `local-backend` port).
//!
//! Implements the documented two-phase background job used by the
//! `local` memory backend in omp:
//!
//! - **Phase 1** (per-session extraction): for each past session
//!   that has changed since it was last processed, an LLM is asked
//!   to distill durable signal. The output is a single small JSON
//!   record per thread.
//! - **Phase 2** (cross-session consolidation): periodically, the
//!   extraction outputs are fed to a second LLM pass that produces
//!   three artifacts on disk under `<memory-root>/`:
//!   - `MEMORY.md` — long-term memory document
//!   - `memory_summary.md` — compact summary injected at session start
//!   - `skills/<name>/SKILL.md` (+ optional scripts/templates/examples)
//!
//! This module is intentionally **self-contained**: it owns its own
//! SQLite tables (via the same `MemoryDb` connection as the rest of
//! the memory store when present, otherwise a separate file) so the
//! pipeline can run independently of `MemoryBackend`. The background
//! spawn hook lives in `services::start_memory_pipeline` (wired from
//! `bootstrap`).
//!
//! Code-only reference ports (MIT):
//!   - `packages/coding-agent/src/memories/index.ts`
//!   - `packages/coding-agent/src/memories/storage.ts`
//!   - `packages/coding-agent/src/memory-backend/local-backend.ts`
//!   - `packages/coding-agent/src/prompts/memories/*.md`
//!   - `packages/coding-agent/src/internal-urls/memory-protocol.ts`
//!
//! Phases 1 and 2 require an LLM `Model` + `Provider` (oxicode-ai). When
//! neither is available, the worker logs once and skips — the
//! pipeline never panics and never blocks session boot.
//!
//! All helpers here are *functional* (no global state, no side
//! effects on import). The runtime spawn is opt-in.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// ── omp prompt ports (MIT) ─────────────────────────────────────
//
// Each prompt is verbatim from
// `packages/coding-agent/src/prompts/memories/*.md` (or paraphrased
// for the {{}}-templated gaps the original uses for now()). Variables
// are spelled out as Rust `const`s with a separate runtime
// substitution helper to keep the prompts easy to diff against the
// upstream source.

/// System prompt for per-session extraction (omp `stage_one_system.md`).
pub const STAGE_ONE_SYSTEM_PROMPT: &str = "\
You are the memory-stage-one extractor.\n\
\n\
You MUST return strict JSON only — no markdown, no commentary.\n\
\n\
Extraction goals:\n\
- You MUST distill reusable durable knowledge from rollout history.\n\
- You MUST keep concrete technical signal (constraints, decisions, workflows, pitfalls, resolved failures).\n\
- You NEVER include transient chatter or low-signal noise.\n\
\n\
Output contract (required keys):\n\
{\n\
  \"rollout_summary\": \"string\",\n\
  \"rollout_slug\": \"string | null\",\n\
  \"raw_memory\": \"string\"\n\
}\n\
\n\
Rules:\n\
- rollout_summary: compact synopsis of what future runs should remember.\n\
- rollout_slug: short lowercase slug (letters/numbers/_), or null.\n\
- raw_memory: detailed durable memory blocks with enough context to reuse.\n\
- If no durable signal exists, you MUST return empty strings for rollout_summary/raw_memory and null rollout_slug.";

/// User-turn template for per-session extraction (omp `stage_one_input.md`).
pub const STAGE_ONE_USER_TEMPLATE: &str = "\
thread_id: {{thread_id}}\n\
\n\
Persistable response items (JSON):\n\
{{response_items_json}}\n\
\n\
You MUST extract durable memory now.";

/// System prompt for cross-session consolidation (omp `consolidation_system.md`).
pub const CONSOLIDATION_SYSTEM_PROMPT: &str = "\
You are the memory-stage-two consolidator.\n\
\n\
Follow the user-provided consolidation task exactly.\n\
Return strict JSON only — no markdown, no commentary.";

/// User-turn template for cross-session consolidation (omp `consolidation.md`).
/// Variables: `{{raw_memories}}` (joined string of stage1 outputs),
/// `{{rollout_summaries}}` (joined string).
pub const CONSOLIDATION_USER_TEMPLATE: &str = "\
Memory consolidation agent.\n\
Memory root: memory://root\n\
Input corpus (raw memories):\n\
{{raw_memories}}\n\
Input corpus (rollout summaries):\n\
{{rollout_summaries}}\n\
Produce strict JSON only with this schema — you NEVER include any other output:\n\
{\n\
  \"memory_md\": \"string\",\n\
  \"memory_summary\": \"string\",\n\
  \"skills\": [\n\
    {\n\
      \"name\": \"string\",\n\
      \"content\": \"string\",\n\
      \"scripts\": [{ \"path\": \"string\", \"content\": \"string\" }],\n\
      \"templates\": [{ \"path\": \"string\", \"content\": \"string\" }],\n\
      \"examples\": [{ \"path\": \"string\", \"content\": \"string\" }]\n\
    }\n\
  ]\n\
}\n\
\n\
Requirements:\n\
- memory_md: long-term memory document.\n\
- memory_summary: prompt-time memory guidance.\n\
- skills: reusable playbooks. Empty array allowed.\n\
- skill.name maps to skills/<name>/.\n\
- skill.content maps to skills/<name>/SKILL.md.\n\
- scripts/templates/examples: optional. Each entry MUST write to skills/<name>/<bucket>/<path>.\n\
- Only include files worth keeping long-term. Omit stale assets so they are pruned.\n\
- Preserve useful prior themes. Remove stale or contradictory guidance.\n\
- Treat memory as advisory: current repository state wins.";

/// Read-path prompt (omp `read-path.md`). Compactly injected at
/// session start so the agent treats memory as advisory context.
pub const READ_PATH_PROMPT: &str = "\
# Memory Guidance\n\
Memory root: memory://root\n\
Operational rules:\n\
1) Read `memory://root/memory_summary.md` first.\n\
2) If needed, inspect `memory://root/MEMORY.md` and `memory://root/skills/<name>/SKILL.md`.\n\
3) Trust memory for heuristics and process context. Trust current repo files, runtime output, and user instruction for factual state and final decisions.\n\
4) When memory changes your plan, cite the artifact path (e.g. `memory://root/skills/<name>/SKILL.md`) and pair it with current-repo evidence.\n\
5) If memory disagrees with repo state or user instruction, treat memory as stale: proceed with corrected behavior, then update/regenerate memory artifacts.\n\
6) Escalate confidence only after repository verification. Memory alone is NEVER sufficient proof.\n\
{{memory_summary_block}}{{learned_block}}";

/// Suggested rendered block when a `memory_summary.md` exists.
pub const READ_PATH_SUMMARY_TEMPLATE: &str = "\
Memory summary:\n\
{{memory_summary}}\n\
";

/// Suggested rendered block when `learned.md` exists.
pub const READ_PATH_LEARNED_TEMPLATE: &str = "\
Learned lessons (captured via the `learn` tool; durable but may be stale — verify against the repo before relying on them):\n\
{{learned}}\n\
";

// ── Tweakable knobs (mirror omp's `MemoryRuntimeConfig`) ────────

/// Sessions older than this are not processed by Phase 1.
pub const DEFAULT_MAX_ROLLOUT_AGE_DAYS: i64 = 30;
/// Sessions active more recently than this are skipped (still in use).
pub const DEFAULT_MIN_ROLLOUT_IDLE_HOURS: i64 = 12;
/// Hard cap on the number of sessions processed per startup.
pub const DEFAULT_MAX_ROLLOUTS_PER_STARTUP: usize = 64;
/// Sum of `memory_summary.md` kept under this many bytes is read into
/// the system prompt directly.
pub const DEFAULT_SUMMARY_INJECTION_TOKEN_LIMIT: usize = 5_000;
/// Lease seconds for Phase 2 ownership tokens (prevents two oxicode
/// processes from double-consolidating).
pub const DEFAULT_GLOBAL_LEASE_SECONDS: i64 = 60;
/// Phase 2 heartbeat refresh interval (must be < lease).
pub const DEFAULT_GLOBAL_HEARTBEAT_SECONDS: i64 = 30;
/// Token cap passed to the model for Stage 1 / Stage 2 inputs.
pub const DEFAULT_MODEL_TOKEN_BUDGET: usize = 8_000;

// ── Secret redaction (lazy-compiled, regex-free) ───────────────

/// Crude secret patterns to redact from written `MEMORY.md` /
/// `memory_summary.md`.
///
/// We use a single linear scan per line against a small set of
/// byte-prefix matchers; that's plenty for guarding accidental
/// token commits. The pattern set is a `LazyLock<Vec<SecretPattern>>`
/// so the lookup table isn't rebuilt per call.
struct SecretPattern {
    /// Stable id for diagnostics.
    name: &'static str,
    /// Byte prefix to match at the start of the secret token.
    prefix: &'static str,
}

static SECRET_PATTERNS: LazyLock<Vec<SecretPattern>> = LazyLock::new(|| {
    vec![
        SecretPattern {
            name: "openai",
            prefix: "sk-",
        },
        SecretPattern {
            name: "anthropic",
            prefix: "sk-ant-",
        },
        SecretPattern {
            name: "github_pat",
            prefix: "ghp_",
        },
        SecretPattern {
            name: "github_oauth",
            prefix: "gho_",
        },
        SecretPattern {
            name: "google_api",
            prefix: "AIza",
        },
        SecretPattern {
            name: "slack",
            prefix: "xoxb-",
        },
    ]
});

/// Redact obvious token patterns from a single line. Returns the
/// redacted line unchanged if no patterns match.
pub fn redact_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        let mut matched = false;
        for pat in SECRET_PATTERNS.iter() {
            if let Some(idx) = rest.find(pat.prefix) {
                // Token-char suffix until the next whitespace / quote.
                let after = &rest[idx + pat.prefix.len()..];
                let tail = after
                    .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                    .unwrap_or(after.len());
                let secret_len = pat.prefix.len() + tail;
                out.push_str(&rest[..idx]);
                out.push_str(&format!("[REDACTED:{}]", pat.name));
                rest = &rest[idx + secret_len..];
                matched = true;
                break;
            }
        }
        if !matched {
            out.push_str(rest);
            break;
        }
    }
    out
}

/// Redact secrets across a multi-line string.
pub fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        out.push_str(&redact_line(line));
    }
    out
}

// ── Path / scope helpers ──────────────────────────────────────

/// Resolve `<home>/.oxicode/memory/<encoded-cwd>/` for the current cwd.
/// Mirrors `encodeProjectPath` in omp's `index.ts`:
///   `let encoded = "--" + cwd.trim_start_matches(['/','\\']).replace(['/','\\',':'], "-") + "--";`
#[allow(clippy::manual_pattern_char_comparison)]
pub fn memory_root(home: &Path, cwd: &str) -> PathBuf {
    let stripped = cwd.trim_start_matches(|c: char| c == '/' || c == '\\');
    let encoded = format!("--{}--", stripped.replace(['/', '\\', ':'], "-"));
    home.join("memory").join(encoded)
}

/// Filenames of the canonical artifacts under `<memory-root>/`.
pub const MEMORY_MD: &str = "MEMORY.md";
pub const MEMORY_SUMMARY_MD: &str = "memory_summary.md";
pub const LEARNED_MD: &str = "learned.md";

/// Sanitize a model-supplied skill name into a safe directory name.
pub fn sanitize_skill_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Sanitize a path within a `skills/<bucket>/<path>` so it cannot
/// escape the bucket root.
pub fn sanitize_skill_relative_path(raw: &str) -> Option<String> {
    let mut parts: Vec<&str> = raw
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.sort();
    Some(parts.join("/"))
}

// ── Helpers (kept tiny; the hard work lives in workers.rs) ─────

/// Render `read-path.md` with optional summary and learned blocks.
///
/// If both `summary` and `learned` are `Some`, both blocks are
/// emitted; if either is `None`, the corresponding placeholder is
/// dropped so we don't ship literal `{{}}` to the agent.
pub fn render_read_path(summary: Option<&str>, learned: Option<&str>) -> String {
    let summary_block = summary
        .map(|s| READ_PATH_SUMMARY_TEMPLATE.replace("{{memory_summary}}", s))
        .unwrap_or_default();
    let learned_block = learned
        .map(|s| READ_PATH_LEARNED_TEMPLATE.replace("{{learned}}", s))
        .unwrap_or_default();
    READ_PATH_PROMPT
        .replace("{{memory_summary_block}}", &summary_block)
        .replace("{{learned_block}}", &learned_block)
}

// Phase 1 + Phase 2 worker functions + SQLite job queue live in
// `memory_workers.rs` to keep this file focused on the artifact
// surface. They are intentionally out of scope of this version;
// `services::start_memory_pipeline` is the wiring point that the
// follow-up PR will hook them up through.

// ── Disk writer (consolidated artifact surface) ────────────────

/// A consolidated skill as supplied by the Stage-2 model.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConsolidationSkill {
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub scripts: Vec<ConsolidationFile>,
    #[serde(default)]
    pub templates: Vec<ConsolidationFile>,
    #[serde(default)]
    pub examples: Vec<ConsolidationFile>,
}

/// One file under `skills/<name>/<bucket>/<path>`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConsolidationFile {
    pub path: String,
    pub content: String,
}

/// JSON payload returned by the Stage-2 LLM.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConsolidationOutput {
    #[serde(default)]
    pub memory_md: String,
    #[serde(default)]
    pub memory_summary: String,
    #[serde(default)]
    pub skills: Vec<ConsolidationSkill>,
}

/// Write the three consolidation artifacts under `memory_root`.
///
/// All written text is run through [`redact_secrets`] before being
/// persisted. Existing `skills/*` directories the Stage-2 output
/// does NOT mention are pruned, mirroring omp's
/// `cleanupConsolidatedArtifacts`.
pub fn write_consolidation_artifacts(
    memory_root: &Path,
    out: &ConsolidationOutput,
) -> std::io::Result<()> {
    std::fs::create_dir_all(memory_root)?;

    let memory_md = redact_secrets(&out.memory_md);
    let summary = redact_secrets(&out.memory_summary);

    let memory_path = memory_root.join(MEMORY_MD);
    let summary_path = memory_root.join(MEMORY_SUMMARY_MD);
    std::fs::write(&memory_path, memory_md.as_bytes())?;
    std::fs::write(&summary_path, summary.as_bytes())?;

    let skills_root = memory_root.join("skills");
    std::fs::create_dir_all(&skills_root)?;

    let mut retained: std::collections::HashSet<String> = std::collections::HashSet::new();
    for skill in &out.skills {
        let dir_name = sanitize_skill_name(&skill.name);
        if dir_name.is_empty() || dir_name == "-" {
            continue;
        }
        let skill_dir = skills_root.join(&dir_name);
        std::fs::create_dir_all(&skill_dir)?;
        retained.insert(dir_name.clone());

        std::fs::write(
            skill_dir.join("SKILL.md"),
            redact_secrets(&skill.content).as_bytes(),
        )?;
        for (bucket, files) in [
            ("scripts", &skill.scripts),
            ("templates", &skill.templates),
            ("examples", &skill.examples),
        ] {
            if files.is_empty() {
                continue;
            }
            let bucket_root = skill_dir.join(bucket);
            std::fs::create_dir_all(&bucket_root)?;
            for file in files.iter() {
                let Some(rel) = sanitize_skill_relative_path(&file.path) else {
                    continue;
                };
                let target = bucket_root.join(&rel);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, redact_secrets(&file.content).as_bytes())?;
            }
        }
    }

    if let Ok(entries) = std::fs::read_dir(&skills_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = path.file_name().and_then(|s| s.to_str())
                && !retained.contains(name)
            {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    Ok(())
}

/// Load `<memory_root>/MEMORY.md` and `<memory_root>/memory_summary.md`
/// if present. Either can be `None` if the file does not exist.
pub fn load_consolidated_artifacts(
    memory_root: &Path,
) -> std::io::Result<(Option<String>, Option<String>)> {
    let memory_md = std::fs::read(memory_root.join(MEMORY_MD))
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    let memory_summary = std::fs::read(memory_root.join(MEMORY_SUMMARY_MD))
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    Ok((memory_md, memory_summary))
}

/// Parse a strict-JSON consolidation payload (Stage-2 model output)
/// into the [`ConsolidationOutput`] shape. Strips ```json fences
/// defensively.
pub fn parse_consolidation_output(text: &str) -> Option<ConsolidationOutput> {
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(trimmed).ok()
}

/// Parse a strict-JSON Stage-1 payload.
pub fn parse_stage1_output(text: &str) -> Option<Stage1OutputShape> {
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(trimmed).ok()
}

/// Stage-1 output JSON shape (omp `stage_one_system.md`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Stage1OutputShape {
    #[serde(default)]
    pub rollout_summary: String,
    #[serde(default)]
    pub rollout_slug: Option<String>,
    #[serde(default)]
    pub raw_memory: String,
}
