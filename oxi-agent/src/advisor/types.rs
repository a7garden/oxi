//! Advisor core types — ported from omp `advise-tool.ts`.
//!
//! Pure data: severity, note, delivery channel. No host/runtime dependency.
//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), which is MIT licensed
//! (Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük).
//! oxi's translation remains under oxi's own MIT license.

use serde::{Deserialize, Serialize};

/// Advisor note severity. omp `AdvisorSeverity`. Omitting it (in the tool
/// schema) is treated as a plain `nit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdvisorSeverity {
    #[default]
    /// Non-urgent cleanup, refactor, missed opportunity. Rides the aside queue.
    Nit,
    /// Agent might be heading wrong or missed something material. Interrupts.
    Concern,
    /// Stop and reconsider. Interrupts.
    Blocker,
}

impl AdvisorSeverity {
    /// Lowercase id matching omp's schema strings.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AdvisorSeverity::Nit => "nit",
            AdvisorSeverity::Concern => "concern",
            AdvisorSeverity::Blocker => "blocker",
        }
    }

    /// Parse a lowercase id. `None` for anything else.
    #[must_use]
    pub fn from_id(s: &str) -> Option<Self> {
        Some(match s {
            "nit" => AdvisorSeverity::Nit,
            "concern" => AdvisorSeverity::Concern,
            "blocker" => AdvisorSeverity::Blocker,
            _ => return None,
        })
    }

    /// Dedupe rank — `nit < concern < blocker`. A new call passes the dedupe
    /// gate only when its rank strictly exceeds the recorded one (a real
    /// escalation). omp `ADVISOR_SEVERITY_RANK`.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            AdvisorSeverity::Nit => 1,
            AdvisorSeverity::Concern => 2,
            AdvisorSeverity::Blocker => 3,
        }
    }
}

/// One queued advice note. omp `AdvisorNote`.
#[derive(Debug, Clone)]
pub struct AdvisorNote {
    /// The advice text, terse and specific.
    pub note: String,
    /// Severity; `None` means a plain nit (omp treats an omitted severity as nit).
    pub severity: Option<AdvisorSeverity>,
}

impl AdvisorNote {
    /// Rank for dedupe. `None` severity defers to `nit` (omp `advisorSeverityRank`).
    #[must_use]
    pub fn rank(&self) -> u8 {
        self.severity.unwrap_or_default().rank()
    }
}

/// How an advisor note reaches the primary agent. omp `AdvisorDeliveryChannel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvisorDeliveryChannel {
    /// Non-interrupting `<advisory>` card rendered in the transcript; the run
    /// continues. Used for `nit`, and for `concern`/`blocker` during the
    /// post-interrupt immune-turn window.
    Aside,
    /// Injected into the primary agent's steering queue — acted on at the next
    /// turn boundary, or (while streaming) mid-turn.
    Steer,
    /// Visible card, but does not resume a stopped run. Used after a deliberate
    /// user interrupt while the agent is idle/aborting.
    Preserve,
}

/// Inputs to [`crate::advisor::channels::resolve_delivery_channel`].
/// omp `resolveAdvisorDeliveryChannel` opts.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeliveryOpts {
    /// The note's severity (`None` = nit).
    pub severity: Option<AdvisorSeverity>,
    /// Latched `true` when the user deliberately interrupted; suppresses
    /// `concern`/`blocker` auto-resume until the user next resumes.
    pub auto_resume_suppressed: bool,
    /// Whether the primary agent is currently streaming a turn.
    pub streaming: bool,
    /// Whether an interrupted turn is still being torn down.
    pub aborting: bool,
    /// Whether the post-interrupt immune-turn cooldown window is active.
    pub interrupt_immune_turn_active: bool,
}

/// omp `ADVISOR_GUIDANCE` — behavioral framing carried as a tag attribute on
/// every rendered `<advisory>` block. The primary agent's system prompt never
/// mentions advisories, so this is its only cue for how to treat them.
pub const ADVISOR_GUIDANCE: &str = "weigh, don't blindly obey";

/// Dedupe key for an advisor note — trim + collapse internal whitespace.
/// omp `advisorNoteDedupeKey`. (Phonetic/content normalization for the
/// *emission* gate lives in [`crate::advisor::emission_guard`].)
#[must_use]
pub(crate) fn note_dedupe_key(note: &str) -> String {
    note.split_whitespace().collect::<Vec<_>>().join(" ")
}
