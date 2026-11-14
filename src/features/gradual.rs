//! Gradual configuration rollout for A/B testing.
//!
//! Allows rolling out configuration changes to a percentage of requests
//! before fully committing.

use crate::core::HotswapConfig;
use crate::error::{ConfigError, Result};
use arc_swap::ArcSwap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// FNV-1a hash for deterministic, cross-process consistent hashing.
/// Unlike `DefaultHasher`, this produces the same output across restarts and replicas.
pub(crate) fn fnv1a_hash(key: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

struct RolloutSnapshot<T> {
    stable: Arc<T>,
    canary: Option<Arc<T>>,
    percentage: u8,
}

/// Gradual rollout state for A/B testing configuration changes.
///
/// Maintains an atomic snapshot of `(stable, canary, percentage)` using [`ArcSwap`]
/// so readers are wait-free and never observe torn state across concurrent transitions.
pub struct GradualRollout<T> {
    state: Arc<ArcSwap<RolloutSnapshot<T>>>,
    write_lock: Arc<Mutex<()>>,
    parent: Option<HotswapConfig<T>>,
}

impl<T: fmt::Debug> fmt::Debug for GradualRollout<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snap = self.state.load();
        f.debug_struct("GradualRollout")
            .field("stable", &*snap.stable)
            .field("canary", &snap.canary.as_deref())
            .field("percentage", &snap.percentage)
            .field("has_parent", &self.parent.is_some())
            .finish()
    }
}

impl<T: Clone> GradualRollout<T> {
    /// Create a new standalone gradual rollout with a stable configuration.
    ///
    /// # Arguments
    ///
    /// * `stable` - The current stable configuration
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hotswap_config::features::GradualRollout;
    /// use std::sync::Arc;
    ///
    /// let rollout: GradualRollout<i32> = GradualRollout::new(Arc::new(42));
    /// ```
    pub fn new(stable: Arc<T>) -> Self {
        Self {
            state: Arc::new(ArcSwap::from_pointee(RolloutSnapshot {
                stable,
                canary: None,
                percentage: 0,
            })),
            write_lock: Arc::new(Mutex::new(())),
            parent: None,
        }
    }

    fn with_parent(stable: Arc<T>, parent: HotswapConfig<T>) -> Self {
        Self {
            state: Arc::new(ArcSwap::from_pointee(RolloutSnapshot {
                stable,
                canary: None,
                percentage: 0,
            })),
            write_lock: Arc::new(Mutex::new(())),
            parent: Some(parent),
        }
    }

    /// Set the canary configuration and rollout percentage (clamped to `0..=100`).
    ///
    /// # Arguments
    ///
    /// * `canary` - The new configuration to test
    /// * `percentage` - Percentage of requests that should use canary (0-100)
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use hotswap_config::features::GradualRollout;
    /// # use std::sync::Arc;
    /// # async fn example() {
    /// let rollout: GradualRollout<i32> = GradualRollout::new(Arc::new(42));
    ///
    /// // Start with 10% rollout
    /// rollout.set_canary(Arc::new(100), 10).await;
    /// # }
    /// ```
    pub async fn set_canary(&self, canary: Arc<T>, percentage: u8) {
        let percentage = percentage.min(100);
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let snap = self.state.load();
        self.state.store(Arc::new(RolloutSnapshot {
            stable: Arc::clone(&snap.stable),
            canary: Some(canary),
            percentage,
        }));
    }

    /// Increase the canary rollout percentage without `u8` overflow (clamped to `100`).
    ///
    /// # Arguments
    ///
    /// * `delta` - Amount to increase percentage by
    ///
    /// Returns the new percentage.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use hotswap_config::features::GradualRollout;
    /// # use std::sync::Arc;
    /// # async fn example() {
    /// let rollout: GradualRollout<i32> = GradualRollout::new(Arc::new(42));
    /// rollout.set_canary(Arc::new(100), 10).await;
    ///
    /// // Increase to 20%
    /// rollout.increase_percentage(10).await;
    /// # }
    /// ```
    pub async fn increase_percentage(&self, delta: u8) -> u8 {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let snap = self.state.load();
        let next = snap.percentage.saturating_add(delta).min(100);
        self.state.store(Arc::new(RolloutSnapshot {
            stable: Arc::clone(&snap.stable),
            canary: snap.canary.clone(),
            percentage: next,
        }));
        next
    }

    /// Promote the canary to stable.
    ///
    /// If this `GradualRollout` was created from a [`HotswapConfig`] via
    /// [`GradualRolloutExt::enable_gradual_rollout`], promoting first updates
    /// (and validates!) the parent `HotswapConfig` before swapping the rollout's
    /// stable snapshot and clearing the canary.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no canary configuration or if parent validation fails.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use hotswap_config::features::GradualRollout;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let rollout: GradualRollout<i32> = GradualRollout::new(Arc::new(42));
    /// rollout.set_canary(Arc::new(100), 50).await;
    ///
    /// // Promote canary to stable
    /// rollout.promote().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn promote(&self) -> Result<()> {
        if let Some(parent) = &self.parent {
            let canary_config = self.state.load().canary.clone().ok_or_else(|| {
                ConfigError::Other("No canary configuration to promote".to_string())
            })?;

            // Validate and commit to parent HotswapConfig first (outside std::sync::Mutex)
            parent.update((*canary_config).clone()).await?;

            let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            self.state.store(Arc::new(RolloutSnapshot {
                stable: canary_config,
                canary: None,
                percentage: 0,
            }));
            Ok(())
        } else {
            let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let snap = self.state.load();
            let canary_config = snap.canary.clone().ok_or_else(|| {
                ConfigError::Other("No canary configuration to promote".to_string())
            })?;

            self.state.store(Arc::new(RolloutSnapshot {
                stable: canary_config,
                canary: None,
                percentage: 0,
            }));
            Ok(())
        }
    }

    /// Rollback by discarding the canary configuration atomically.
    ///
    /// All requests will immediately use the stable configuration.
    pub async fn rollback_canary(&self) {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let snap = self.state.load();
        self.state.store(Arc::new(RolloutSnapshot {
            stable: Arc::clone(&snap.stable),
            canary: None,
            percentage: 0,
        }));
    }

    /// Synchronously select a configuration using a wait-free atomic snapshot read.
    ///
    /// If `key` is `None`, uses randomized selection (`fastrand`).
    /// If `key` is `Some(k)`, uses deterministic FNV-1a hashing so the same key
    /// always maps to the same bucket across threads, restarts, and replicas.
    pub fn get_sync(&self, key: Option<&str>) -> Arc<T> {
        let snap = self.state.load();

        let Some(canary) = &snap.canary else {
            return Arc::clone(&snap.stable);
        };
        if snap.percentage == 0 {
            return Arc::clone(&snap.stable);
        }
        if snap.percentage >= 100 {
            return Arc::clone(canary);
        }

        let should_use_canary = if let Some(key) = key {
            (fnv1a_hash(key) % 100) < snap.percentage as u64
        } else {
            fastrand::u8(0..100) < snap.percentage
        };

        if should_use_canary {
            Arc::clone(canary)
        } else {
            Arc::clone(&snap.stable)
        }
    }

    /// Get a configuration based on optional key for consistent hashing.
    ///
    /// Delegates to [`Self::get_sync`] for wait-free atomic selection.
    ///
    /// # Arguments
    ///
    /// * `key` - Optional key for consistent hashing (e.g., `user_id`)
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use hotswap_config::features::GradualRollout;
    /// # use std::sync::Arc;
    /// # async fn example() {
    /// let rollout: GradualRollout<i32> = GradualRollout::new(Arc::new(42));
    /// rollout.set_canary(Arc::new(100), 50).await;
    ///
    /// // Random selection
    /// let config = rollout.get(None).await;
    ///
    /// // Consistent hashing by user ID
    /// let config = rollout.get(Some("user123")).await;
    /// # }
    /// ```
    pub async fn get(&self, key: Option<&str>) -> Arc<T> {
        self.get_sync(key)
    }

    /// Get the current rollout percentage.
    pub async fn get_percentage(&self) -> u8 {
        self.state.load().percentage
    }

    /// Check if a canary configuration is currently set.
    pub async fn has_canary(&self) -> bool {
        self.state.load().canary.is_some()
    }

    /// Get the stable configuration.
    pub async fn get_stable(&self) -> Arc<T> {
        Arc::clone(&self.state.load().stable)
    }

    /// Get the canary configuration if set.
    pub async fn get_canary(&self) -> Option<Arc<T>> {
        self.state.load().canary.clone()
    }
}

impl<T> Clone for GradualRollout<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            write_lock: Arc::clone(&self.write_lock),
            parent: self.parent.clone(),
        }
    }
}

/// Extension trait for gradual rollout support on [`HotswapConfig`].
pub trait GradualRolloutExt<T> {
    /// Enable gradual rollout linked to this [`HotswapConfig`].
    ///
    /// Calling [`GradualRollout::promote`] will automatically validate and promote
    /// the canary configuration into the parent `HotswapConfig`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use hotswap_config::features::GradualRolloutExt;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, Clone)]
    /// struct AppConfig {
    ///     port: u16,
    /// }
    ///
    /// # async fn example(config: HotswapConfig<AppConfig>) -> Result<()> {
    /// let rollout = config.enable_gradual_rollout();
    ///
    /// // Set a canary config with 10% rollout
    /// let canary = AppConfig { port: 9090 };
    /// rollout.set_canary(std::sync::Arc::new(canary), 10).await;
    ///
    /// // Increase rollout
    /// rollout.increase_percentage(10).await;
    ///
    /// // Promote to stable (also updates `config`!)
    /// rollout.promote().await?;
    /// # Ok(())
    /// # }
    /// ```
    fn enable_gradual_rollout(&self) -> GradualRollout<T>;
}

impl<T> GradualRolloutExt<T> for HotswapConfig<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn enable_gradual_rollout(&self) -> GradualRollout<T> {
        let current = self.get();
        GradualRollout::with_parent(current, self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gradual_rollout_creation_and_debug() {
        let rollout = GradualRollout::new(Arc::new(42));
        assert_eq!(*rollout.get_stable().await, 42);
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 0);
        assert!(format!("{:?}", rollout).contains("GradualRollout"));
    }

    #[tokio::test]
    async fn test_set_canary() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 50).await;

        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 50);
        assert_eq!(*rollout.get_canary().await.unwrap(), 100);
    }

    #[tokio::test]
    async fn test_percentage_clamping() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 150).await;

        assert_eq!(rollout.get_percentage().await, 100);
    }

    #[tokio::test]
    async fn test_increase_percentage_and_u8_overflow_safety() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 10).await;

        rollout.increase_percentage(20).await;
        assert_eq!(rollout.get_percentage().await, 30);

        // Delta 250 would overflow u8 (30 + 250 = 280 > u8::MAX) without saturating_add!
        let pct = rollout.increase_percentage(250).await;
        assert_eq!(pct, 100);
        assert_eq!(rollout.get_percentage().await, 100);
    }

    #[tokio::test]
    async fn test_promote_standalone_and_parent_hotswap_config() {
        let config = HotswapConfig::new(42);
        let rollout = config.enable_gradual_rollout();
        rollout.set_canary(Arc::new(100), 50).await;

        rollout.promote().await.unwrap();

        assert_eq!(*rollout.get_stable().await, 100);
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 0);
        // Parent HotswapConfig must also reflect the promoted stable config!
        assert_eq!(*config.get(), 100);
    }

    #[tokio::test]
    async fn test_promote_without_canary() {
        let rollout = GradualRollout::new(Arc::new(42));
        let result = rollout.promote().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rollback_canary() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 50).await;

        rollout.rollback_canary().await;

        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 0);
        assert_eq!(*rollout.get_stable().await, 42);
    }

    #[tokio::test]
    async fn test_get_no_canary() {
        let rollout = GradualRollout::new(Arc::new(42));

        for _ in 0..10 {
            let config = rollout.get(None).await;
            assert_eq!(*config, 42);
        }
    }

    #[tokio::test]
    async fn test_get_zero_percent() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 0).await;

        for _ in 0..10 {
            let config = rollout.get(None).await;
            assert_eq!(*config, 42);
        }
    }

    #[tokio::test]
    async fn test_get_hundred_percent() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 100).await;

        for _ in 0..10 {
            let config = rollout.get(None).await;
            assert_eq!(*config, 100);
        }
    }

    #[tokio::test]
    async fn test_get_with_consistent_hashing_and_get_sync() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 50).await;

        let key = "user123";
        let first = rollout.get_sync(Some(key));
        for _ in 0..10 {
            let config = rollout.get(Some(key)).await;
            assert_eq!(*config, *first);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_reads_and_rollout_transitions_never_tear_or_panic() {
        let rollout = GradualRollout::new(Arc::new(1));
        let reader_rollout = rollout.clone();

        let reader = tokio::spawn(async move {
            for i in 0..5_000usize {
                let key = format!("user_{}", i % 50);
                let val = *reader_rollout.get_sync(Some(&key));
                assert!(val == 1 || val == 2);
            }
        });

        for _ in 0..200 {
            rollout.set_canary(Arc::new(2), 50).await;
            rollout.increase_percentage(60).await;
            rollout.rollback_canary().await;
        }

        reader.await.unwrap();
    }

    #[tokio::test]
    async fn test_gradual_rollout_distribution() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 50).await;

        let mut canary_count = 0;
        let iterations = 1000;

        for _ in 0..iterations {
            let config = rollout.get(None).await;
            if *config != 42 {
                canary_count += 1;
            }
        }

        let canary_percentage = (canary_count * 100) / iterations;
        assert!((40..=60).contains(&canary_percentage));
    }
}
