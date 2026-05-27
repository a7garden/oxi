//! Keybinding system for the oxi TUI.
//!
//! Provides declarative key mapping with:
//! - Normalized key representation (`KeyId`)
//! - Action registry with default bindings (`KeybindingsManager`)
//! - User rebinding from config
//! - Conflict detection
//!
//! ## Usage
//!
//! ```rust
//! use oxi_tui::keybindings::{KeybindingsManager, keys::KeyId, registry::Action};
//!
//! let mgr = KeybindingsManager::new();
//!
//! // Look up action from a crossterm KeyEvent
//! let key_id = KeyId::from(event);
//! if let Some(action) = mgr.match_action(&key_id) {
//!     match action {
//!         Action::Submit => { /* ... */ }
//!         Action::Quit => { /* ... */ }
//!         _ => {}
//!     }
//! }
//! ```

pub mod conflict;
pub mod keys;
pub mod registry;

pub use conflict::{detect_conflicts, validate_user_bindings, KeybindingConflict};
pub use keys::{as_char, is_printable, parse_key_id, parse_kitty_sequence, BaseKey, KeyId};
pub use registry::{Action, KeybindingsManager};
