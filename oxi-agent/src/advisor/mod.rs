//! Advisor subsystem — a read-only reviewer agent that shadows the primary
//! agent and surfaces advice (`nit`/`concern`/`blocker`) via an `advise` tool.
//!
//! Ported from omp `packages/coding-agent/src/advisor/`. The **engine** lives
//! here (in oxi-agent) so the SDK can expose it; **host wiring** —
//! `AgentSession` integration, the file-backed transcript recorder, WATCHDOG.md
//! discovery, and the `/advisor` slash command — lives in the product
//! (oxi-cli).
//!
//! # Attribution
//!
//! Translated to Rust from omp (oh-my-pi), which is MIT licensed
//! (Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük).
//! oxi's translation remains under oxi's own MIT license.

pub mod advise_tool;
pub mod agent_advisor;
pub mod channels;
pub mod emission_guard;
pub mod runtime;
pub mod types;

pub use advise_tool::{AdviseTool, EnqueueAdviceFn};
pub use agent_advisor::AgentAdvisor;
pub use channels::{
    format_advisory_batch, is_immune_turn_active, is_interrupting_severity,
    resolve_delivery_channel,
};
pub use emission_guard::{AdvisorEmissionGuard, normalize_advisor_note};
pub use runtime::{AdvisorAgent, AdvisorRuntime, AdvisorRuntimeHost};
pub use types::{
    ADVISOR_GUIDANCE, AdvisorDeliveryChannel, AdvisorNote, AdvisorSeverity, DeliveryOpts,
};

/// Tool names handed to the advisor agent — side-effect-free investigation
/// only. omp `ADVISOR_READONLY_TOOL_NAMES` is `read`/`grep`/`glob`; oxi's glob
/// equivalent is `find`.
pub const ADVISOR_READONLY_TOOL_NAMES: &[&str] = &["read", "grep", "find"];

/// The advisor agent's system prompt (omp `prompts/advisor/system.md`). The
/// host assembles the full advisor system prompt from this + optional
/// context-files/watchdog blocks.
pub const ADVISOR_SYSTEM_PROMPT: &str = include_str!("prompts/system.md");
