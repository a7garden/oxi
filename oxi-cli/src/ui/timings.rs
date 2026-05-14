//! Central timing instrumentation for startup profiling.
//!
//! Enable with the `OXI_TIMING=1` environment variable.
//!
//! Startup profiling and timing instrumentation.

use std::sync::Mutex;
use std::time::Instant;

/// A single recorded timing entry.
#[derive(Debug, Clone)]
pub struct TimingEntry {
/// pub.
    pub label: String,
/// pub.
    pub ms: u64,
}

static TIMINGS: Mutex<Vec<TimingEntry>> = Mutex::new(Vec::new());
static LAST_TIME: Mutex<Option<Instant>> = Mutex::new(None);

fn enabled() -> bool {
    std::env::var("OXI_TIMING").as_deref() == Ok("1")
}

/// Reset all recorded timings.
pub fn reset_timings() {
    if !enabled() {
        return;
    }
    if let Ok(mut timings) = TIMINGS.lock() {
        timings.clear();
    }
    if let Ok(mut last) = LAST_TIME.lock() {
        *last = Some(Instant::now());
    }
}

/// Record a timing checkpoint with the given label.
pub fn time(label: &str) {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let (Ok(mut timings), Ok(mut last)) = (TIMINGS.lock(), LAST_TIME.lock()) {
        let ms = match *last {
            Some(prev) => now.duration_since(prev).as_millis() as u64,
            None => 0,
        };
        timings.push(TimingEntry {
            label: label.to_string(),
            ms,
        });
        *last = Some(now);
    }
}

/// Print all recorded timings to stderr.
pub fn print_timings() {
    if !enabled() {
        return;
    }
    let timings = TIMINGS.lock().unwrap_or_else(|e| e.into_inner());
    if timings.is_empty() {
        return;
    }
    let total: u64 = timings.iter().map(|t| t.ms).sum();
    eprintln!("\n--- Startup Timings ---");
    for t in timings.iter() {
        eprintln!("  {}: {}ms", t.label, t.ms);
    }
    eprintln!("  TOTAL: {}ms", total);
    eprintln!("------------------------\n");
}

/// Get a snapshot of all recorded timing entries (for programmatic access).
pub fn get_timings() -> Vec<TimingEntry> {
    let timings = TIMINGS.lock().unwrap_or_else(|e| e.into_inner());
    timings.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_timings() {
        // Temporarily enable timing
        std::env::set_var("OXI_TIMING", "1");
        reset_timings();
        time("test");
        assert!(!get_timings().is_empty());
        reset_timings();
        assert!(get_timings().is_empty());
        std::env::remove_var("OXI_TIMING");
    }
}
