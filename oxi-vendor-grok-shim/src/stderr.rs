//! Shim of `xai_grok_shared::stderr` — stderr locking utilities.

use std::io::{self, Write};

/// Lock stderr and return a guard.
pub fn stderr_lock() -> io::StderrLock<'static> {
    io::stderr().lock()
}

/// Run a closure with stderr locked.
pub fn with_locked_stderr<F, R>(f: F) -> R
where
    F: FnOnce(&mut io::StderrLock<'static>) -> R,
{
    let mut stderr = io::stderr().lock();
    f(&mut stderr)
}
