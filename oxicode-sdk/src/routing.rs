//! Runtime routing control — dynamic model routing and fallback management.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Runtime routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Whether automatic routing is enabled.
    pub auto_routing: bool,
    /// Prefer cost-efficient models when routing.
    pub prefer_cost_efficient: bool,
    /// Fallback models to try when the primary model fails.
    pub fallback_models: Vec<String>,
    /// Models to exclude from routing (e.g., due to outages).
    pub excluded_models: Vec<String>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            auto_routing: true,
            prefer_cost_efficient: false,
            fallback_models: Vec::new(),
            excluded_models: Vec::new(),
        }
    }
}

/// Runtime routing control interface.
///
/// Allows dynamic toggling of routing, swapping fallback models,
/// and excluding specific models at runtime.
#[derive(Debug, Clone)]
pub struct RoutingControl {
    enabled: Arc<AtomicBool>,
    config: Arc<RwLock<RoutingConfig>>,
}

impl RoutingControl {
    /// Create a new routing control with the given config.
    pub fn new(config: RoutingConfig) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(config.auto_routing)),
            config: Arc::new(RwLock::new(config)),
        }
    }

    /// Create a disabled routing control.
    pub fn disabled() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            config: Arc::new(RwLock::new(RoutingConfig::default())),
        }
    }

    /// Enable or disable routing.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Whether routing is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Update the routing configuration.
    pub fn update_config(&self, f: impl FnOnce(&mut RoutingConfig)) {
        f(&mut self.config.write());
    }

    /// Replace the fallback model list.
    pub fn set_fallback_models(&self, models: Vec<String>) {
        self.config.write().fallback_models = models;
    }

    /// Exclude a specific model from routing.
    pub fn exclude_model(&self, model_id: &str) {
        let mut config = self.config.write();
        if !config.excluded_models.contains(&model_id.to_string()) {
            config.excluded_models.push(model_id.to_string());
        }
    }

    /// Remove a model from the exclusion list.
    pub fn unexclude_model(&self, model_id: &str) {
        self.config
            .write()
            .excluded_models
            .retain(|m| m != model_id);
    }

    /// Get the current routing config.
    pub fn config(&self) -> RoutingConfig {
        self.config.read().clone()
    }

    /// Get the fallback models.
    pub fn fallback_models(&self) -> Vec<String> {
        self.config.read().fallback_models.clone()
    }

    /// Get the excluded models.
    pub fn excluded_models(&self) -> Vec<String> {
        self.config.read().excluded_models.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_control_default() {
        let rc = RoutingControl::new(RoutingConfig::default());
        assert!(rc.is_enabled());
    }

    #[test]
    fn routing_control_toggle() {
        let rc = RoutingControl::new(RoutingConfig::default());
        rc.set_enabled(false);
        assert!(!rc.is_enabled());
        rc.set_enabled(true);
        assert!(rc.is_enabled());
    }

    #[test]
    fn routing_control_disabled() {
        let rc = RoutingControl::disabled();
        assert!(!rc.is_enabled());
    }

    #[test]
    fn routing_control_fallback_models() {
        let rc = RoutingControl::new(RoutingConfig::default());
        rc.set_fallback_models(vec!["model-a".into(), "model-b".into()]);
        assert_eq!(rc.fallback_models().len(), 2);
    }

    #[test]
    fn routing_control_exclude_model() {
        let rc = RoutingControl::new(RoutingConfig::default());
        rc.exclude_model("bad-model");
        assert!(rc.excluded_models().contains(&"bad-model".to_string()));
        rc.unexclude_model("bad-model");
        assert!(!rc.excluded_models().contains(&"bad-model".to_string()));
    }

    #[test]
    fn routing_control_update_config() {
        let rc = RoutingControl::new(RoutingConfig::default());
        rc.update_config(|c| {
            c.prefer_cost_efficient = true;
        });
        assert!(rc.config().prefer_cost_efficient);
    }

    #[test]
    fn routing_control_no_duplicate_exclusion() {
        let rc = RoutingControl::new(RoutingConfig::default());
        rc.exclude_model("model-1");
        rc.exclude_model("model-1");
        assert_eq!(rc.excluded_models().len(), 1);
    }

    /// Mutations through one clone are visible through another —
    /// this is what makes RoutingControl "live" across the
    /// supervisor / handle / resolver boundary. The Arc-backed
    /// inner state means callers can hold a clone and observe
    /// runtime reconfiguration without re-fetching.
    #[test]
    fn routing_control_live_across_clones() {
        let rc = RoutingControl::new(RoutingConfig::default());
        let observer = rc.clone();

        // Mutate via the original.
        rc.set_enabled(false);
        rc.exclude_model("primary-model");
        rc.set_fallback_models(vec!["fallback-a".into(), "fallback-b".into()]);

        // Observe via the clone — state is shared, not copied.
        assert!(
            !observer.is_enabled(),
            "set_enabled must propagate to clones"
        );
        assert!(
            observer
                .excluded_models()
                .contains(&"primary-model".to_string()),
            "exclude_model must propagate to clones"
        );
        assert_eq!(
            observer.fallback_models().len(),
            2,
            "set_fallback_models must propagate to clones"
        );

        // Reverse direction works too.
        observer.unexclude_model("primary-model");
        assert!(
            !rc.excluded_models().contains(&"primary-model".to_string()),
            "unexclude_model via clone must propagate back"
        );
    }

    /// The host's resolver can call `config()` at provider-resolution
    /// time to read the live config snapshot. This test verifies the
    /// returned `RoutingConfig` is a consistent point-in-time copy
    /// (snapshot semantics — not a live reference).
    #[test]
    fn routing_control_config_snapshot_is_point_in_time() {
        let rc = RoutingControl::new(RoutingConfig::default());
        let snap = rc.config();
        rc.exclude_model("later-exclusion");
        // snap was taken BEFORE the mutation — must not reflect it.
        assert!(
            !snap
                .excluded_models
                .contains(&"later-exclusion".to_string())
        );
        // A fresh read reflects it.
        assert!(
            rc.excluded_models()
                .contains(&"later-exclusion".to_string())
        );
    }
}
