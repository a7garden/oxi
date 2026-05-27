//! Agent execution metrics.
//!
//! Provides atomic counters for tracking agent runtime statistics:
//! runs, tokens, tool calls, durations.

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic agent metrics, safe for concurrent updates.
#[derive(Debug, Default)]
pub struct AgentMetrics {
    /// Total number of agent runs.
    pub total_runs: AtomicU64,
    /// Successful runs.
    pub successful_runs: AtomicU64,
    /// Failed runs.
    pub failed_runs: AtomicU64,
    /// Total input (prompt) tokens consumed.
    pub total_input_tokens: AtomicU64,
    /// Total output (completion) tokens consumed.
    pub total_output_tokens: AtomicU64,
    /// Total tokens consumed (input + output).
    pub total_tokens: AtomicU64,
    /// Total tool calls made.
    pub tool_calls: AtomicU64,
    /// Cumulative duration in milliseconds.
    pub total_duration_ms: AtomicU64,
}

impl AgentMetrics {
    /// Create a new zero-initialized metrics instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful run.
    pub fn record_success(
        &self,
        duration_ms: u64,
        input_tokens: u64,
        output_tokens: u64,
        tool_count: u64,
    ) {
        self.total_runs.fetch_add(1, Ordering::Relaxed);
        self.successful_runs.fetch_add(1, Ordering::Relaxed);
        self.total_input_tokens
            .fetch_add(input_tokens, Ordering::Relaxed);
        self.total_output_tokens
            .fetch_add(output_tokens, Ordering::Relaxed);
        self.total_tokens
            .fetch_add(input_tokens + output_tokens, Ordering::Relaxed);
        self.tool_calls.fetch_add(tool_count, Ordering::Relaxed);
        self.total_duration_ms
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    /// Record a failed run.
    pub fn record_failure(&self, duration_ms: u64) {
        self.total_runs.fetch_add(1, Ordering::Relaxed);
        self.failed_runs.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ms
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    /// Take a snapshot of all counters.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            total_runs: self.total_runs.load(Ordering::Relaxed),
            successful_runs: self.successful_runs.load(Ordering::Relaxed),
            failed_runs: self.failed_runs.load(Ordering::Relaxed),
            total_input_tokens: self.total_input_tokens.load(Ordering::Relaxed),
            total_output_tokens: self.total_output_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
            total_duration_ms: self.total_duration_ms.load(Ordering::Relaxed),
        }
    }

    /// Reset all counters to zero.
    pub fn reset(&self) {
        self.total_runs.store(0, Ordering::Relaxed);
        self.successful_runs.store(0, Ordering::Relaxed);
        self.failed_runs.store(0, Ordering::Relaxed);
        self.total_input_tokens.store(0, Ordering::Relaxed);
        self.total_output_tokens.store(0, Ordering::Relaxed);
        self.total_tokens.store(0, Ordering::Relaxed);
        self.tool_calls.store(0, Ordering::Relaxed);
        self.total_duration_ms.store(0, Ordering::Relaxed);
    }
}

/// Point-in-time snapshot of agent metrics.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MetricsSnapshot {
    /// Total number of agent runs.
    pub total_runs: u64,
    /// Successful runs.
    pub successful_runs: u64,
    /// Failed runs.
    pub failed_runs: u64,
    /// Total input (prompt) tokens consumed.
    #[serde(default)]
    pub total_input_tokens: u64,
    /// Total output (completion) tokens consumed.
    #[serde(default)]
    pub total_output_tokens: u64,
    /// Total tokens consumed (input + output).
    pub total_tokens: u64,
    /// Total tool calls made.
    pub tool_calls: u64,
    /// Cumulative duration in milliseconds.
    pub total_duration_ms: u64,
}

impl MetricsSnapshot {
    /// Calculate the success rate (0.0 to 1.0).
    pub fn success_rate(&self) -> f64 {
        if self.total_runs == 0 {
            return 0.0;
        }
        self.successful_runs as f64 / self.total_runs as f64
    }

    /// Calculate the average run duration in milliseconds.
    pub fn avg_duration_ms(&self) -> f64 {
        if self.total_runs == 0 {
            return 0.0;
        }
        self.total_duration_ms as f64 / self.total_runs as f64
    }

    /// Calculate the average tokens per run.
    pub fn avg_tokens(&self) -> f64 {
        if self.total_runs == 0 {
            return 0.0;
        }
        self.total_tokens as f64 / self.total_runs as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_snapshot_empty() {
        let metrics = AgentMetrics::new();
        let snap = metrics.snapshot();
        assert_eq!(snap.total_runs, 0);
        assert_eq!(snap.success_rate(), 0.0);
    }

    #[test]
    fn test_metrics_record_success() {
        let metrics = AgentMetrics::new();
        metrics.record_success(100, 300, 200, 3);
        metrics.record_success(200, 500, 300, 5);

        let snap = metrics.snapshot();
        assert_eq!(snap.total_runs, 2);
        assert_eq!(snap.successful_runs, 2);
        assert_eq!(snap.failed_runs, 0);
        assert_eq!(snap.total_input_tokens, 800);
        assert_eq!(snap.total_output_tokens, 500);
        assert_eq!(snap.total_tokens, 1300);
        assert_eq!(snap.tool_calls, 8);
        assert_eq!(snap.total_duration_ms, 300);
        assert!((snap.success_rate() - 1.0).abs() < f64::EPSILON);
        assert!((snap.avg_duration_ms() - 150.0).abs() < f64::EPSILON);
        assert!((snap.avg_tokens() - 650.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_metrics_record_failure() {
        let metrics = AgentMetrics::new();
        metrics.record_failure(50);

        let snap = metrics.snapshot();
        assert_eq!(snap.total_runs, 1);
        assert_eq!(snap.failed_runs, 1);
        assert!((snap.success_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_metrics_reset() {
        let metrics = AgentMetrics::new();
        metrics.record_success(100, 300, 200, 3);
        metrics.reset();

        let snap = metrics.snapshot();
        assert_eq!(snap.total_runs, 0);
        assert_eq!(snap.total_input_tokens, 0);
        assert_eq!(snap.total_output_tokens, 0);
    }

    #[test]
    fn test_snapshot_serialization() {
        let metrics = AgentMetrics::new();
        metrics.record_success(100, 300, 200, 3);
        let snap = metrics.snapshot();

        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"total_runs\":1"));
        assert!(json.contains("\"total_input_tokens\":300"));
        assert!(json.contains("\"total_output_tokens\":200"));
    }

    #[test]
    fn test_snapshot_deserialize_backward_compat() {
        // Old JSON without new fields should deserialize with defaults
        let old_json = r#"{"total_runs":5,"successful_runs":4,"failed_runs":1,"total_tokens":50000,"tool_calls":20,"total_duration_ms":30000}"#;
        let snap: MetricsSnapshot = serde_json::from_str(old_json).unwrap();
        assert_eq!(snap.total_runs, 5);
        assert_eq!(snap.total_tokens, 50000);
        assert_eq!(snap.total_input_tokens, 0);
        assert_eq!(snap.total_output_tokens, 0);
    }
}
