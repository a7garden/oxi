//! OXI-CHANGE: heavily stubbed. Upstream `terminal::overlay` coordinates
//! image/preview overlays with terminal graphics protocols; out of scope
//! for oxi's text/markdown render port. Only the `PostFlush` type is kept
//! because `draw.rs` references it in its function signatures and calls
//! `.write_to(...)` on it. We expose a no-op `write_to`.
#![allow(dead_code)]

use std::io::{self, Write};

use ratatui::layout::Rect;

/// Post-flush overlay hook. Stubbed — draw.rs holds one but it never
/// writes anything in oxi's text-only render path.
#[derive(Default, Clone, Copy)]
pub struct PostFlush;

impl PostFlush {
    /// OXI-CHANGE: no-op. Upstream wrote image-clear escapes; oxi renders
    /// text/markdown only, so there is nothing to flush.
    pub fn write_to<W: Write>(&self, _writer: &mut W) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum Ownership {
    Anonymous,
    Owned(u64),
}

pub(crate) fn clear_overlay(_owner: Ownership) -> io::Result<()> {
    Ok(())
}

pub(crate) fn write_post_flush<W: Write>(_writer: &mut W, _area: Rect) -> io::Result<()> {
    Ok(())
}
