//! Input system: terminal keyboard, paste, and kill-ring handling.
//!
//! Standalone module added to `oxi-tui` as part of the realignment with
//! `omp/packages/tui`. Currently unused by the existing render path or
//! `oxi-cli` — provides protocol-level primitives that any future input
//! pipeline can pick up.
//!
//! ## Submodules
//!
//! - `kitty` — Kitty keyboard protocol (CSI-u) parser and legacy xterm
//!   fallback matcher. Produces `kitty::ParsedKey` values with full
//!   modifier and press/repeat/release semantics.
//! - `paste` — Bracketed-paste-mode handler that buffers bytes between
//!   `\x1b[200~` and `\x1b[201~` markers, emitting `paste::PasteEvent`s
//!   for state transitions. Tolerates markers split across reads and
//!   caps memory at a configurable byte limit.
//! - `kill_ring` — Emacs-style kill ring for `yank` / `yank-pop` text
//!   cycling in editors. Bounded FIFO ring with configurable capacity.

pub mod kill_ring;
pub mod kitty;
pub mod paste;

pub use kill_ring::KillRing;
pub use kitty::{
    KeyEventKind, KeyModifiers, KeyType, ParsedKey, matches_legacy_key, parse_kitty_key,
};
pub use paste::{DEFAULT_BYTE_LIMIT, PasteBuffer, PasteEvent};
