//! Stale detection for extension contexts.
//!
//! When a session switches, forks, or reloads, all extension contexts
//! from the previous session become stale. The guard uses an atomic
//! generation counter to detect this efficiently.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Shared generation counter for invalidation.
/// Create one per session. When the session changes,
/// call `invalidate()` to bump the generation.
#[derive(Debug, Clone)]
pub struct InvalidationToken {
    inner: Arc<AtomicU64>,
}

impl InvalidationToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Bump the generation, invalidating all existing guards.
    pub fn invalidate(&self) {
        self.inner.fetch_add(1, Ordering::Release);
    }

    /// Create a guard that tracks the current generation.
    pub fn guard(&self) -> ContextGuard {
        ContextGuard {
            generation: self.inner.load(Ordering::Acquire),
            token: self.inner.clone(),
        }
    }

    /// Current generation value.
    pub fn generation(&self) -> u64 {
        self.inner.load(Ordering::Acquire)
    }
}

impl Default for InvalidationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Guard that checks whether its context is still valid.
#[derive(Debug, Clone)]
pub struct ContextGuard {
    generation: u64,
    token: Arc<AtomicU64>,
}

impl ContextGuard {
    /// Check if this guard's context is still valid.
    pub fn is_valid(&self) -> bool {
        self.generation == self.token.load(Ordering::Acquire)
    }

    /// Assert validity, returning an error if stale.
    pub fn check_valid(&self) -> anyhow::Result<()> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Extension context is stale — session has been switched or reloaded"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guard_starts_valid() {
        let token = InvalidationToken::new();
        let guard = token.guard();
        assert!(guard.is_valid());
        assert!(guard.check_valid().is_ok());
    }

    #[test]
    fn test_invalidation_invalidates_guard() {
        let token = InvalidationToken::new();
        let guard = token.guard();
        assert!(guard.is_valid());

        token.invalidate();
        assert!(!guard.is_valid());
        assert!(guard.check_valid().is_err());
    }

    #[test]
    fn test_new_guard_after_invalidation_is_valid() {
        let token = InvalidationToken::new();
        token.invalidate();

        let new_guard = token.guard();
        assert!(new_guard.is_valid());
    }

    #[test]
    fn test_multiple_invalidations() {
        let token = InvalidationToken::new();
        let g0 = token.guard();
        token.invalidate();
        let g1 = token.guard();
        token.invalidate();
        let g2 = token.guard();

        assert!(!g0.is_valid());
        assert!(!g1.is_valid());
        assert!(g2.is_valid());
    }
}
