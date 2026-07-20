// PagerEvent — normalized input from agent / user / tick / background.
//
// Only `AgentEvent` crosses the oxi-agent boundary; crossterm events,
// ticks, and background-job notifications are wrapped locally. The
// `emitter` module owns the type definitions; `main_loop` (PR-4) wires
// the actual sources.

use oxi_agent::events::AgentEvent;

/// All inputs to the reducer go through this enum.
#[derive(Debug, Clone)]
pub enum PagerEvent {
    Agent(Box<AgentEvent>),
    Input(ResolvedKey),
    Tick,
    Background(BackgroundEvent),
}

/// Resolved key — populated by the KeyRouter (PR-3). For now a stub
/// carrying the raw event; PR-3 will replace with the modal/global
/// dispatch enum.
#[derive(Debug, Clone)]
pub enum ResolvedKey {
    /// Pass-through to the focused widget (used in PR-2 before the
    /// KeyRouter is introduced).
    PassThrough(crossterm::event::KeyEvent),
    /// Ignored (no binding, no modal).
    Ignored,
}

/// Placeholder for subagent / MCP completions arriving after the owning
/// turn has ended. Filled out in PR-5.
#[derive(Debug, Clone)]
pub enum BackgroundEvent {
    Stub,
}
