//! Panel widgets — chrome around the chat surface.
//!
//! - [`Footer`] — one-line status bar (model · tokens · cost · spinner).
//! - [`Sticky`] — top- or bottom-anchored band that paints a bg fill.
//! - [`Overlay`] — centered modal with border + optional title.

pub mod footer;
pub mod overlay;
pub mod sticky;

pub use footer::Footer;
pub use overlay::Overlay;
pub use sticky::{Sticky, StickyPosition};
