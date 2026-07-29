//! Agent Hub overlay — fullscreen TUI monitor for advisor + subagents.
//!
//! Modules here land incrementally across Tasks 3, 6, 7, and 8 of the
//! advisor + agent hub plan. `transcript` (this task) is the first piece;
//! the rest (`state`, `table`, `keys`) arrive once Task 6 wires them.

pub mod transcript;
