//! Temporal expression parsing and decay — ported from omp
//! `core/temporal-parser.ts` and `core/weibull.ts`.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};

/// A parsed temporal expression from a query.
#[derive(Debug, Clone)]
pub struct TemporalExpr {
    pub timestamp: DateTime<Utc>,
    pub text: String,
}

/// Extract temporal expressions from a query string.
pub fn extract_temporal(query: &str) -> Vec<TemporalExpr> {
    let lower = query.to_lowercase();
    let now = Utc::now();
    let mut results = Vec::new();

    for (keyword, delta_days) in [("today", 0), ("yesterday", -1), ("tomorrow", 1)] {
        if lower.contains(keyword) {
            results.push(TemporalExpr {
                timestamp: now + Duration::days(delta_days),
                text: keyword.to_string(),
            });
        }
    }

    for word in lower.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| !c.is_ascii_digit() && c != '-');
        if let Ok(date) = NaiveDate::parse_from_str(cleaned, "%Y-%m-%d")
            && let Some(ts) = date.and_hms_opt(0, 0, 0)
        {
            results.push(TemporalExpr {
                timestamp: DateTime::<Utc>::from_naive_utc_and_offset(ts, Utc),
                text: cleaned.to_string(),
            });
        }
    }

    let months = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    for (i, &month) in months.iter().enumerate() {
        if lower.contains(month) {
            let ts = Utc
                .with_ymd_and_hms(now.year(), (i + 1) as u32, 1, 0, 0, 0)
                .single()
                .unwrap_or(now);
            results.push(TemporalExpr {
                timestamp: ts,
                text: month.to_string(),
            });
        }
    }

    results
}

/// Exponential temporal boost: `boost = exp(-diff_hours / halflife)`.
pub fn temporal_boost(memory_time: &str, query_time: DateTime<Utc>, halflife_hours: f64) -> f32 {
    let parsed = DateTime::parse_from_rfc3339(memory_time)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(memory_time, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| DateTime::<Utc>::from_naive_utc_and_offset(t, Utc))
        });

    let Some(mem_dt) = parsed else { return 0.0 };
    let diff_hours = (query_time - mem_dt).num_seconds().abs() as f64 / 3600.0;
    ((-diff_hours / halflife_hours.max(0.001)).exp()) as f32
}

/// Whether a query asks about "current" state.
pub fn query_asks_current(query: &str) -> bool {
    let lower = query.to_lowercase();
    [
        "now",
        "current",
        "currently",
        "latest",
        "recent",
        "today",
        "active",
        "present",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// Content adjustment for current-sensitive queries.
pub fn current_content_adjustment(content: &str, current_sensitive: bool) -> f32 {
    if !current_sensitive {
        return 1.0;
    }
    let lower = content.to_lowercase();
    let mut factor = 1.0;
    if ["current", "currently", "latest", "now", "active", "present"]
        .iter()
        .any(|w| lower.contains(w))
    {
        factor *= 1.35;
    }
    if [
        "was",
        "previous",
        "previously",
        "legacy",
        "old",
        "stale",
        "former",
        "deprecated",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        factor *= 0.72;
    }
    factor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_today() {
        let r = extract_temporal("what happened today");
        assert!(r.iter().any(|t| t.text == "today"));
    }

    #[test]
    fn extract_iso_date() {
        let r = extract_temporal("deployed on 2024-01-15");
        assert!(r.iter().any(|t| t.text == "2024-01-15"));
    }

    #[test]
    fn boost_exact_time() {
        let now = Utc::now();
        let boost = temporal_boost(&now.to_rfc3339(), now, 72.0);
        assert!((boost - 1.0).abs() < 0.01);
    }

    #[test]
    fn boost_old_time() {
        let now = Utc::now();
        let old = now - Duration::days(30);
        let boost = temporal_boost(&old.to_rfc3339(), now, 72.0);
        assert!(boost < 0.1);
    }
}
