//! View state and row projection for the Agent Hub overlay.
//!
//! Holds the rows displayed in the table (a snapshot of [`HubRegistry`]
//! precomputed for rendering), the current view ([`HubView`]), selection,
//! and the transcript-view scroll + tail-follow state.
//!
//! `HubState::from_registry` is the one place that turns a registry snapshot
//! into render-ready [`HubRow`]s — including the age-text formatting that
//! the table column shows.

use std::collections::HashMap;
use std::path::PathBuf;

use oxi_sdk::{HubKind, HubStatus};

use crate::app::agent_hub_registry::HubRegistry;

/// One row in the Agent Hub table, precomputed for rendering.
#[derive(Debug, Clone)]
pub struct HubRow {
    pub id: String,
    pub kind: HubKind,
    pub status: HubStatus,
    pub display_name: String,
    pub current_task: Option<String>,
    /// Pre-formatted "Ns ago" / "Nm ago" / "Nh ago" string.
    pub age_text: String,
    /// On-disk JSONL file the transcript reader tails, if known.
    pub session_file: Option<PathBuf>,
}

/// Active view inside the overlay.
#[derive(Debug, Clone)]
pub enum HubView {
    /// List of agents — the default landing screen.
    Table,
    /// Live transcript tail for one agent id.
    Transcript { agent_id: String },
}

/// All mutable overlay state.
#[derive(Debug)]
pub struct HubState {
    /// Rows shown in the table, in registry-snapshot order.
    pub rows: Vec<HubRow>,
    pub view: HubView,
    /// Index into `rows` for the highlighted table row.
    pub selected: usize,
    /// Stable id → row-index map so selection survives row reordering.
    /// Currently informational — kept per the plan surface so future PRs
    /// don't break the contract.
    pub row_order: HashMap<String, usize>,
    /// Top-line offset for the transcript viewport, signed so handlers can
    /// step from the tail without knowing the line count.
    ///   `0`             — follow tail (the `FOLLOW_TAIL` sentinel)
    ///   `> 0`           — lines from the head (top of history)
    ///   `< 0`           — `|scroll|` lines below the tail (toward history)
    ///   `isize::MAX`     — saturate to the top of history (`g`)
    /// The renderer clamps to a valid window based on the current line count.
    pub transcript_scroll: isize,
    /// When true, the transcript renderer pins to the last line on every
    /// poll so the user sees fresh output without manual scroll.
    pub transcript_follow: bool,
}

impl HubState {
    /// Project a registry snapshot into render-ready rows. The registry
    /// already returns rows sorted by `(status.sort_key, last_activity desc)`;
    /// we copy the order verbatim and just expand the age text.
    pub fn from_registry(reg: &HubRegistry, now_ms: u64) -> Vec<HubRow> {
        reg.snapshot()
            .into_iter()
            .map(|(id, e)| HubRow {
                age_text: e.age_text(now_ms),
                id,
                kind: e.kind,
                status: e.status,
                display_name: e.display_name,
                current_task: e.current_task,
                session_file: e.session_file,
            })
            .collect()
    }
}
