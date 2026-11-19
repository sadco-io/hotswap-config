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
    /// **Note on validation:** Canaries set with `set_canary` are served immediately
    /// to `percentage`% of selections and are **not** validated until [`Self::promote`]
    /// is called — even when this `GradualRollout` is linked to a parent
    /// [`HotswapConfig`] with a registered validator. Use [`Self::try_set_canary`]
    /// to validate a canary before serving it.
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

    /// Validate and set the canary configuration and rollout percentage (clamped to `0..=100`).
    ///
    /// Behavior depends on whether this `GradualRollout` is linked to a parent [`HotswapConfig`]:
    ///
    /// - **Linked** (created via [`GradualRolloutExt::enable_gradual_rollout`]): validates
    ///   `canary` using [`HotswapConfig::validate`] before installing it. If validation
    ///   fails, returns an error and leaves the rollout state (stable, any existing canary,
    ///   and percentage) unchanged. Because this is a dry-run check, rejections are not
    ///   counted in the parent's validation-failure metric (`hotswap_config.validation.failures`).
    /// - **Unlinked / standalone** (created via [`Self::new`]): no parent validator exists,
    ///   so this behaves identically to [`Self::set_canary`] and returns `Ok(())`.
    ///
    /// # Arguments
    ///
    /// * `canary` - The new configuration to test
    /// * `percentage` - Percentage of requests that should use canary (0-100)
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ValidationError`] when linked to a parent [`HotswapConfig`]
    /// whose registered validator(s) reject `canary`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use hotswap_config::features::GradualRollout;
    /// # use std::sync::Arc;
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// let rollout: GradualRollout<i32> = GradualRollout::new(Arc::new(42));
    ///
    /// // Start with 10% rollout
    /// rollout.try_set_canary(Arc::new(100), 10).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn try_set_canary(&self, canary: Arc<T>, percentage: u8) -> Result<()> {
        if let Some(parent) = &self.parent {
            parent.validate(&canary)?;
        }
        self.set_canary(canary, percentage).await;
        Ok(())
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
    /// stable snapshot and clearing the canary. If a different canary was
    /// installed while the parent update was in flight, it and its percentage
    /// are kept, and only `stable` is replaced with the parent's committed
    /// config.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no canary configuration or if parent validation fails.
    ///
    /// On parent validation failure, the canary stays installed at its current rollout
    /// percentage and continues to be served by [`Self::get`] and [`Self::get_sync`];
    /// call [`Self::rollback_canary`] to discard it and stop serving it.
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
            let snap = self.state.load();
            let same_canary = snap
                .canary
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &canary_config));
            let (canary, percentage) = if same_canary {
                (None, 0)
            } else {
                // Keep a newer canary (or rollback) installed while parent.update was awaiting.
                (snap.canary.clone(), snap.percentage)
            };
            self.state.store(Arc::new(RolloutSnapshot {
                stable: parent.get(),
                canary,
                percentage,
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
    /// Use [`GradualRollout::try_set_canary`] to validate a candidate canary against
    /// the parent [`HotswapConfig`] before serving it (whereas
    /// [`GradualRollout::set_canary`] defers validation until
    /// [`GradualRollout::promote`]). Calling [`GradualRollout::promote`] will
    /// automatically validate and promote the canary configuration into the parent
    /// `HotswapConfig`.
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
    /// // Validate and set a canary config with 10% rollout
    /// let canary = AppConfig { port: 9090 };
    /// rollout.try_set_canary(std::sync::Arc::new(canary), 10).await?;
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

    #[derive(Debug, Clone, PartialEq, serde::Deserialize)]
    struct CanaryTestConfig {
        port: u16,
    }

    #[cfg(feature = "validation")]
    impl crate::core::Validate for CanaryTestConfig {
        fn validate(&self) -> std::result::Result<(), crate::error::ValidationError> {
            if self.port < 1024 {
                return Err(crate::error::ValidationError::invalid_field(
                    "port",
                    "must be >= 1024",
                ));
            }
            Ok(())
        }
    }

    fn canary_memory_source(port: i64) -> impl crate::sources::ConfigSource {
        use std::collections::HashMap;

        struct MemorySource(i64);

        impl crate::sources::ConfigSource for MemorySource {
            fn load(&self) -> Result<HashMap<String, config::Value>> {
                let mut map = HashMap::new();
                map.insert("port".to_string(), config::Value::from(self.0));
                Ok(map)
            }

            fn name(&self) -> String {
                "canary-memory".to_string()
            }

            fn priority(&self) -> i32 {
                100
            }
        }

        MemorySource(port)
    }

    #[tokio::test]
    async fn test_try_set_canary_rejects_invalid_and_preserves_state() {
        use crate::error::ValidationError;

        let config = HotswapConfig::builder()
            .with_source(canary_memory_source(8080))
            .with_validation(|cfg: &CanaryTestConfig| {
                if cfg.port < 1024 {
                    return Err(ValidationError::invalid_field("port", "must be >= 1024"));
                }
                Ok(())
            })
            .build::<CanaryTestConfig>()
            .await
            .unwrap();

        let rollout = config.enable_gradual_rollout();

        // Reject invalid canary when no canary is installed
        let err = rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 80 }), 50)
            .await;
        assert!(matches!(err, Err(ConfigError::ValidationError(_))));
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 0);
        assert!(rollout.get_canary().await.is_none());

        for i in 0..1_000 {
            let key = format!("user-{}", i);
            assert_eq!(rollout.get_sync(Some(&key)).port, 8080);
            assert_eq!(rollout.get(None).await.port, 8080);
        }

        // Install a valid canary, then verify a subsequent invalid try_set_canary leaves it intact
        rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 9090 }), 25)
            .await
            .unwrap();
        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 25);

        let err = rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 80 }), 100)
            .await;
        assert!(matches!(err, Err(ConfigError::ValidationError(_))));
        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 25);
        assert_eq!(rollout.get_canary().await.unwrap().port, 9090);

        for i in 0..1_000 {
            let key = format!("user-{}", i);
            let keyed = rollout.get_sync(Some(&key)).port;
            let random = rollout.get(None).await.port;
            assert!(keyed == 8080 || keyed == 9090);
            assert!(random == 8080 || random == 9090);
        }
    }

    #[tokio::test]
    async fn test_try_set_canary_valid_served_at_percentage_and_promotes() {
        use crate::error::ValidationError;

        let config = HotswapConfig::builder()
            .with_source(canary_memory_source(8080))
            .with_validation(|cfg: &CanaryTestConfig| {
                if cfg.port < 1024 {
                    return Err(ValidationError::invalid_field("port", "must be >= 1024"));
                }
                Ok(())
            })
            .build::<CanaryTestConfig>()
            .await
            .unwrap();

        let rollout = config.enable_gradual_rollout();
        rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 9090 }), 25)
            .await
            .unwrap();

        let mut keyed_canary_hits = 0usize;
        let mut random_canary_hits = 0usize;
        let iterations = 1_000usize;
        for i in 0..iterations {
            let key = format!("user-{}", i);
            if rollout.get_sync(Some(&key)).port == 9090 {
                keyed_canary_hits += 1;
            }
            if rollout.get(None).await.port == 9090 {
                random_canary_hits += 1;
            }
        }
        assert!((150..=350).contains(&keyed_canary_hits));
        assert!((150..=350).contains(&random_canary_hits));

        rollout.promote().await.unwrap();
        assert_eq!(config.get().port, 9090);
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_sync(Some("user-0")).port, 9090);
    }

    #[cfg(feature = "validation")]
    #[tokio::test]
    async fn test_try_set_canary_with_validate_trait() {
        let config = HotswapConfig::builder()
            .with_source(canary_memory_source(8080))
            .build_validated::<CanaryTestConfig>()
            .await
            .unwrap();

        let rollout = config.enable_gradual_rollout();

        let err = rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 443 }), 50)
            .await;
        assert!(matches!(err, Err(ConfigError::ValidationError(_))));
        assert!(!rollout.has_canary().await);

        rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 8443 }), 100)
            .await
            .unwrap();
        assert_eq!(rollout.get(None).await.port, 8443);
    }

    #[tokio::test]
    async fn test_try_set_canary_unlinked_behaves_like_set_canary() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.try_set_canary(Arc::new(100), 150).await.unwrap();

        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 100);
        assert_eq!(*rollout.get(None).await, 100);
    }

    #[tokio::test]
    async fn test_set_canary_and_failed_promote_keeps_canary_until_rollback() {
        use crate::error::ValidationError;

        let config = HotswapConfig::builder()
            .with_source(canary_memory_source(8080))
            .with_validation(|cfg: &CanaryTestConfig| {
                if cfg.port < 1024 {
                    return Err(ValidationError::invalid_field("port", "must be >= 1024"));
                }
                Ok(())
            })
            .build::<CanaryTestConfig>()
            .await
            .unwrap();

        let rollout = config.enable_gradual_rollout();
        rollout
            .set_canary(Arc::new(CanaryTestConfig { port: 80 }), 100)
            .await;
        assert_eq!(rollout.get(None).await.port, 80);

        let err = rollout.promote().await;
        assert!(matches!(err, Err(ConfigError::ValidationError(_))));
        // Failed promote preserves the canary until rollback_canary() is called
        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get(None).await.port, 80);
        assert_eq!(config.get().port, 8080);

        rollout.rollback_canary().await;
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get(None).await.port, 8080);
    }

    #[tokio::test]
    async fn test_try_set_canary_linked_without_validator_succeeds() {
        let config = HotswapConfig::new(42);
        let rollout = config.enable_gradual_rollout();

        rollout.try_set_canary(Arc::new(100), 50).await.unwrap();
        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 50);
        assert_eq!(*rollout.get_canary().await.unwrap(), 100);
        assert_eq!(*config.get(), 42);

        let built = HotswapConfig::builder()
            .with_source(canary_memory_source(8080))
            .build::<CanaryTestConfig>()
            .await
            .unwrap();
        let built_rollout = built.enable_gradual_rollout();
        built_rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 80 }), 25)
            .await
            .unwrap();
        assert!(built_rollout.has_canary().await);
        assert_eq!(built_rollout.get_percentage().await, 25);
        assert_eq!(built_rollout.get_canary().await.unwrap().port, 80);
    }

    #[tokio::test]
    async fn test_linked_try_set_canary_clamps_percentage() {
        use crate::error::ValidationError;

        let config = HotswapConfig::builder()
            .with_source(canary_memory_source(8080))
            .with_validation(|cfg: &CanaryTestConfig| {
                if cfg.port < 1024 {
                    return Err(ValidationError::invalid_field("port", "must be >= 1024"));
                }
                Ok(())
            })
            .build::<CanaryTestConfig>()
            .await
            .unwrap();

        let rollout = config.enable_gradual_rollout();
        rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 9090 }), 150)
            .await
            .unwrap();

        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 100);
        assert_eq!(rollout.get(None).await.port, 9090);
        assert_eq!(rollout.get_sync(Some("user-0")).port, 9090);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_promote_preserves_concurrently_installed_canary() {
        use crate::error::ValidationError;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;

        let armed = Arc::new(AtomicBool::new(false));
        let (start_tx, start_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let start_tx = Arc::new(Mutex::new(Some(start_tx)));
        let done_rx = Arc::new(Mutex::new(done_rx));

        let armed_clone = Arc::clone(&armed);
        let start_tx_clone = Arc::clone(&start_tx);
        let done_rx_clone = Arc::clone(&done_rx);

        let config = HotswapConfig::builder()
            .with_source(canary_memory_source(8080))
            .with_validation(move |cfg: &CanaryTestConfig| {
                if cfg.port < 1024 {
                    return Err(ValidationError::invalid_field("port", "must be >= 1024"));
                }
                if armed_clone.swap(false, Ordering::SeqCst) {
                    if let Some(tx) = start_tx_clone.lock().unwrap().take() {
                        tx.send(()).unwrap();
                    }
                    tokio::task::block_in_place(|| {
                        done_rx_clone.lock().unwrap().recv().unwrap();
                    });
                }
                Ok(())
            })
            .build::<CanaryTestConfig>()
            .await
            .unwrap();

        let rollout = config.enable_gradual_rollout();
        rollout
            .try_set_canary(Arc::new(CanaryTestConfig { port: 9090 }), 25)
            .await
            .unwrap();

        let rollout_for_worker = rollout.clone();
        let concurrent_setter = tokio::spawn(async move {
            tokio::task::block_in_place(|| {
                start_rx.recv().unwrap();
            });
            rollout_for_worker
                .try_set_canary(Arc::new(CanaryTestConfig { port: 9091 }), 40)
                .await
                .unwrap();
            rollout_for_worker.increase_percentage(15).await;
            done_tx.send(()).unwrap();
        });

        armed.store(true, Ordering::SeqCst);
        rollout.promote().await.unwrap();
        concurrent_setter.await.unwrap();

        // Promoted canary (9090) became stable in both parent and rollout,
        // while the concurrently installed canary (9091 at 55%) was preserved.
        assert_eq!(config.get().port, 9090);
        assert_eq!(rollout.get_stable().await.port, 9090);
        assert!(rollout.has_canary().await);
        assert_eq!(rollout.get_canary().await.unwrap().port, 9091);
        assert_eq!(rollout.get_percentage().await, 55);
    }

    #[cfg(feature = "file-watch")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_concurrent_linked_promote_keeps_latest_parent_stable() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;

        let armed = Arc::new(AtomicBool::new(true));
        let (stalled_tx, stalled_rx) = mpsc::channel::<()>();
        let (resume_tx, resume_rx) = mpsc::channel::<()>();
        let stalled_tx = Arc::new(Mutex::new(Some(stalled_tx)));
        let resume_rx = Arc::new(Mutex::new(resume_rx));

        let config = HotswapConfig::new(1);
        let armed_clone = Arc::clone(&armed);
        let stalled_tx_clone = Arc::clone(&stalled_tx);
        let resume_rx_clone = Arc::clone(&resume_rx);
        let _sub = config
            .subscribe(move || {
                if armed_clone.swap(false, Ordering::SeqCst) {
                    if let Some(tx) = stalled_tx_clone.lock().unwrap().take() {
                        tx.send(()).unwrap();
                    }
                    tokio::task::block_in_place(|| {
                        resume_rx_clone.lock().unwrap().recv().unwrap();
                    });
                }
            })
            .await;

        let rollout = config.enable_gradual_rollout();
        rollout.set_canary(Arc::new(2), 50).await;

        let rollout_for_worker = rollout.clone();
        let second_promote = tokio::spawn(async move {
            tokio::task::block_in_place(|| {
                stalled_rx.recv().unwrap();
            });
            rollout_for_worker.set_canary(Arc::new(3), 100).await;
            rollout_for_worker.promote().await.unwrap();
            resume_tx.send(()).unwrap();
        });

        rollout.promote().await.unwrap();
        second_promote.await.unwrap();

        assert_eq!(*config.get(), 3);
        assert_eq!(*rollout.get_stable().await, *config.get());
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 0);
    }
}
