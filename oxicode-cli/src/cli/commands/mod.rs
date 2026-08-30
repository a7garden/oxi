//! CLI subcommand handlers.
//!
//! Each submodule owns the handler(s) for a family of `Commands` variants.
//! `main.rs` dispatches the match arm to the appropriate function here.

pub mod config;
pub mod doctor;
pub mod export;
pub mod ext;
pub mod issue;
pub mod migrate;
pub mod misc;
pub mod pkg;
pub mod reset;
pub mod sessions;
pub mod setup;
pub use config::*;
pub use doctor::*;
pub use export::*;
pub use ext::*;
pub use issue::*;
pub use migrate::*;
pub use misc::*;
pub use pkg::*;
pub use reset::*;
pub use sessions::*;
pub use setup::*;
