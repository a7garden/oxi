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
//! ```rust,no_run
//! use crossterm::event::{KeyEvent, KeyCode, KeyModifiers};
//! use oxi_tui::keybindings::{KeybindingsManager, keys::KeyId, registry::Action};
//!
//! let mgr = KeybindingsManager::new();
//!
//! // Look up action from a crossterm KeyEvent
//! let event = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
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

pub use conflict::{KeybindingConflict, detect_conflicts, validate_user_bindings};
pub use keys::{BaseKey, KeyId, as_char, is_printable, parse_key_id, parse_kitty_sequence};
pub use registry::{Action, KeybindingsManager};
