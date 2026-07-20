//! OXI-CHANGE: stub. Upstream `link_opener` opens URLs in the host browser
//! via `xai_grok_tools` / `xai_tty_utils`; not relevant to oxi's render path.
//! `is_safe_to_open` is referenced from `osc8.rs` for hyperlink filtering —
//! return `false` to be safe (no auto-open). Caller treats it as a hint.
#![allow(dead_code, unused_variables)]

use crate::render::grok::terminal::hyperlinks::SchemeFilter;

pub fn is_safe_to_open(_url: &str, _filter: SchemeFilter) -> bool {
    false
}
