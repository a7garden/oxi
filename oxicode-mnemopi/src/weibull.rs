//! Weibull decay — per-memory-type retention curves.
//!
//! Ported from omp `core/weibull.ts`. Each memory type (profile, preference,
//! fact, etc.) has Weibull parameters (k=shape, eta=scale in hours) that
//! control how quickly its recall boost decays over time. Higher eta = slower
//! decay; lower k = more long-term retention.
//!
//! MIT — attribution: adapted from [omp](https://github.com/earendil-works/pi)
//! `packages/mnemopi/src/core/weibull.ts`.

use chrono::{DateTime, Utc};

/// Weibull parameters for a memory type.
#[derive(Debug, Clone, Copy)]
pub struct WeibullParams {
    /// Shape parameter. Lower k = more long-term retention.
    pub k: f64,
    /// Scale parameter in hours. Higher eta = slower decay.
    pub eta: f64,
}

/// Per-memory-type Weibull parameters.
pub fn params_for(memory_type: &str) -> Option<WeibullParams> {
    let (k, eta) = match memory_type {
        "profile" => (0.3, 8_760.0),
        "preference" => (0.4, 4_380.0),
        "relationship" => (0.35, 8_760.0),
        "learning" => (0.7, 1_440.0),
        "fact" => (0.8, 720.0),
        "entity" => (0.5, 4_380.0),
        "setup" => (0.6, 2_160.0),
        "pattern" => (0.6, 1_680.0),
        "context" => (0.85, 360.0),
        "observation" => (0.9, 480.0),
        "artifact" => (0.75, 2_160.0),
        "project" => (0.85, 1_080.0),
        "goal" => (0.9, 720.0),
        "decision" => (1.0, 336.0),
        "commitment" => (1.0, 240.0),
        "event" => (1.2, 168.0),
        "instruction" => (0.9, 480.0),
        "error" => (1.1, 336.0),
        "issue" => (1.1, 336.0),
        "request" => (1.5, 72.0),
        "general" => (1.0, 168.0),
        _ => return None,
    };
    Some(WeibullParams { k, eta })
}

/// Default half-life in hours (1 week).
pub const DEFAULT_HALFLIFE_HOURS: f64 = 168.0;

/// Parse a timestamp string (RFC3339 or date-only) into a UTC `DateTime`.
fn parse_timestamp(timestamp: &str) -> Option<DateTime<Utc>> {
    // Try RFC3339 first
    if let Ok(dt) = DateTime::parse_from_rfc3339(timestamp) {
        return Some(dt.with_timezone(&Utc));
    }
    // Try date-only YYYY-MM-DD
    if timestamp.len() == 10
        && let Ok(date) = chrono::NaiveDate::parse_from_str(timestamp, "%Y-%m-%d")
    {
        return date
            .and_hms_opt(0, 0, 0)
            .map(|naive| naive.and_local_timezone(Utc).unwrap().with_timezone(&Utc));
    }
    // Try naive datetime
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_local_timezone(Utc).unwrap().with_timezone(&Utc));
    }
    None
}

/// Weibull-based recency boost for a memory.
///
/// Returns a value in `[0, 1]`: 1.0 when the memory is fresh, decaying
/// toward 0 as age increases. The decay curve depends on the memory type.
///
/// - `timestamp`: RFC3339 timestamp of the memory.
/// - `query_time`: when the query is happening (default: now).
/// - `memory_type`: controls the decay curve.
/// - `halflife_hours`: if provided, uses simple exponential decay
///   `exp(-age / halflife)` instead of Weibull.
pub fn weibull_boost(
    timestamp: &str,
    query_time: Option<DateTime<Utc>>,
    memory_type: &str,
    halflife_hours: Option<f64>,
) -> f64 {
    let memory_time = match parse_timestamp(timestamp) {
        Some(t) => t,
        None => return 0.0,
    };
    let now = query_time.unwrap_or_else(Utc::now);

    let age_hours = (now - memory_time).num_milliseconds() as f64 / 3_600_000.0;
    weibull_decay_factor(age_hours, memory_type, halflife_hours)
}

/// Weibull decay factor for a given age (in hours).
///
/// Returns 1.0 for age ≤ 0 (future timestamps). When `halflife_hours` is
/// provided, uses simple exponential decay. Otherwise uses the per-type
/// Weibull curve.
pub fn weibull_decay_factor(age_hours: f64, memory_type: &str, halflife_hours: Option<f64>) -> f64 {
    if age_hours <= 0.0 {
        return 1.0;
    }

    if let Some(halflife) = halflife_hours {
        if halflife <= 0.0 {
            return 0.0;
        }
        return (-age_hours / halflife).exp();
    }

    match params_for(memory_type) {
        Some(params) => {
            if params.eta <= 0.0 {
                return 0.0;
            }
            (-((age_hours / params.eta).powf(params.k))).exp()
        }
        None => (-age_hours / DEFAULT_HALFLIFE_HOURS).exp(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_memory_boost_is_one() {
        let now = Utc::now();
        let ts = now.to_rfc3339();
        let boost = weibull_boost(&ts, Some(now), "general", None);
        assert!((boost - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_old_memory_decays() {
        let old = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let boost = weibull_boost(&old, None, "general", None);
        assert!(boost < 0.5, "expected decay, got {boost}");
    }

    #[test]
    fn test_profile_type_slow_decay() {
        let old = (Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let profile_boost = weibull_boost(&old, None, "profile", None);
        let general_boost = weibull_boost(&old, None, "general", None);
        // Profile memories should decay slower than general
        assert!(
            profile_boost > general_boost,
            "profile ({profile_boost}) should decay slower than general ({general_boost})"
        );
    }

    #[test]
    fn test_request_type_fast_decay() {
        let old = (Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
        let request_boost = weibull_boost(&old, None, "request", None);
        let profile_boost = weibull_boost(&old, None, "profile", None);
        assert!(
            request_boost < profile_boost,
            "request ({request_boost}) should decay faster than profile ({profile_boost})"
        );
    }

    #[test]
    fn test_halflife_override() {
        let age = 168.0; // 1 week
        // halflife = 168 hours → exp(-1) ≈ 0.368
        let factor = weibull_decay_factor(age, "general", Some(168.0));
        assert!((factor - (-1.0f64).exp()).abs() < 0.01);
    }

    #[test]
    fn test_unknown_type_uses_default() {
        let factor = weibull_decay_factor(168.0, "unknown_type", None);
        // Should use default halflife
        let expected = (-168.0 / DEFAULT_HALFLIFE_HOURS).exp();
        assert!((factor - expected).abs() < 0.01);
    }

    #[test]
    fn test_future_timestamp() {
        let factor = weibull_decay_factor(-10.0, "general", None);
        assert_eq!(factor, 1.0);
    }

    #[test]
    fn test_invalid_timestamp() {
        let boost = weibull_boost("not-a-date", None, "general", None);
        assert_eq!(boost, 0.0);
    }
}
