//! OXI-CHANGE: stub. Upstream `terminal::image` detects/renders inline
//! graphics protocols (Kitty, iTerm2, Sixel). Out of scope for oxi's
//! text/markdown port. Kept as empty module so `super::image::*` reference
//! paths resolve; image-overlay features simply no-op.
#![allow(dead_code)]

#[cfg(test)]
pub(crate) fn set_protocol_for_test() {}
