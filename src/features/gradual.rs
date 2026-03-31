//! Gradual configuration rollout for A/B testing.
//!
//! Allows rolling out configuration changes to a percentage of requests
//! before fully committing.

use crate::core::HotswapConfig;
use crate::error::{ConfigError, Result};
use std::sync::Arc;

/// FNV-1a hash for deterministic, cross-process consistent hashing.
/// Unlike DefaultHasher, this produces the same output across restarts and replicas.
fn fnv1a_hash(key: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in key.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

use tokio::sync::RwLock;

/// Gradual rollout state for A/B testing configuration changes.
///
/// Maintains two configurations (stable and canary) and selects between them
/// based on a percentage rollout.
pub struct GradualRollout<T> {
    stable: Arc<RwLock<Arc<T>>>,
    canary: Arc<RwLock<Option<Arc<T>>>>,
    percentage: Arc<RwLock<u8>>,
}

impl<T: Clone> GradualRollout<T> {
    /// Create a new gradual rollout with a stable configuration.
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
            stable: Arc::new(RwLock::new(stable)),
            canary: Arc::new(RwLock::new(None)),
            percentage: Arc::new(RwLock::new(0)),
        }
    }

    /// Set the canary configuration and rollout percentage.
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
        *self.canary.write().await = Some(canary);
        *self.percentage.write().await = percentage;
    }

    /// Increase the canary rollout percentage.
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
        let mut percentage = self.percentage.write().await;
        *percentage = (*percentage + delta).min(100);
        *percentage
    }

    /// Promote the canary to stable.
    ///
    /// Replaces the stable configuration with the canary and clears the canary.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no canary configuration.
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
        let mut canary = self.canary.write().await;
        let canary_config = canary
            .take()
            .ok_or_else(|| ConfigError::Other("No canary configuration to promote".to_string()))?;

        *self.stable.write().await = canary_config;
        *self.percentage.write().await = 0;

        Ok(())
    }

    /// Rollback by discarding the canary configuration.
    ///
    /// All requests will use the stable configuration.
    pub async fn rollback_canary(&self) {
        *self.canary.write().await = None;
        *self.percentage.write().await = 0;
    }

    /// Get a configuration based on optional key for consistent hashing.
    ///
    /// If no key is provided, uses random selection.
    /// If a key is provided, uses consistent hashing to ensure the same key
    /// always gets the same configuration.
    ///
    /// # Arguments
    ///
    /// * `key` - Optional key for consistent hashing (e.g., user_id)
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
        let percentage = *self.percentage.read().await;
        let canary = self.canary.read().await;

        // If no canary or 0% rollout, always return stable
        if canary.is_none() || percentage == 0 {
            return Arc::clone(&*self.stable.read().await);
        }

        // If 100% rollout, always return canary
        if percentage == 100 {
            return Arc::clone(canary.as_ref().unwrap());
        }

        // Determine if this request should get canary
        let should_use_canary = if let Some(key) = key {
            // Deterministic hashing — same key always maps to same bucket
            // across restarts and replicas (unlike DefaultHasher)
            let hash = fnv1a_hash(key);
            (hash % 100) < percentage as u64
        } else {
            // Random selection
            fastrand::u8(0..100) < percentage
        };

        if should_use_canary {
            Arc::clone(canary.as_ref().unwrap())
        } else {
            Arc::clone(&*self.stable.read().await)
        }
    }

    /// Get the current rollout percentage.
    pub async fn get_percentage(&self) -> u8 {
        *self.percentage.read().await
    }

    /// Check if a canary configuration is currently set.
    pub async fn has_canary(&self) -> bool {
        self.canary.read().await.is_some()
    }

    /// Get the stable configuration.
    pub async fn get_stable(&self) -> Arc<T> {
        Arc::clone(&*self.stable.read().await)
    }

    /// Get the canary configuration if set.
    pub async fn get_canary(&self) -> Option<Arc<T>> {
        self.canary.read().await.as_ref().map(Arc::clone)
    }
}

impl<T: Clone> Clone for GradualRollout<T> {
    fn clone(&self) -> Self {
        Self {
            stable: Arc::clone(&self.stable),
            canary: Arc::clone(&self.canary),
            percentage: Arc::clone(&self.percentage),
        }
    }
}

/// Extension trait for gradual rollout support on HotswapConfig.
pub trait GradualRolloutExt<T> {
    /// Enable gradual rollout with an initial canary percentage.
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
    /// // Promote to stable
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
        GradualRollout::new(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gradual_rollout_creation() {
        let rollout = GradualRollout::new(Arc::new(42));
        assert_eq!(*rollout.get_stable().await, 42);
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 0);
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
    async fn test_increase_percentage() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 10).await;

        rollout.increase_percentage(20).await;
        assert_eq!(rollout.get_percentage().await, 30);

        rollout.increase_percentage(80).await;
        assert_eq!(rollout.get_percentage().await, 100);
    }

    #[tokio::test]
    async fn test_promote() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 50).await;

        rollout.promote().await.unwrap();

        assert_eq!(*rollout.get_stable().await, 100);
        assert!(!rollout.has_canary().await);
        assert_eq!(rollout.get_percentage().await, 0);
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

        // Should always return stable
        for _ in 0..10 {
            let config = rollout.get(None).await;
            assert_eq!(*config, 42);
        }
    }

    #[tokio::test]
    async fn test_get_zero_percent() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 0).await;

        // Should always return stable
        for _ in 0..10 {
            let config = rollout.get(None).await;
            assert_eq!(*config, 42);
        }
    }

    #[tokio::test]
    async fn test_get_hundred_percent() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 100).await;

        // Should always return canary
        for _ in 0..10 {
            let config = rollout.get(None).await;
            assert_eq!(*config, 100);
        }
    }

    #[tokio::test]
    async fn test_get_with_consistent_hashing() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 50).await;

        // Same key should always return same config
        let key = "user123";
        let first = rollout.get(Some(key)).await;
        for _ in 0..10 {
            let config = rollout.get(Some(key)).await;
            assert_eq!(*config, *first);
        }
    }

    #[tokio::test]
    async fn test_hotswap_config_integration() {
        let config = HotswapConfig::new(42);
        let rollout = config.enable_gradual_rollout();

        assert_eq!(*rollout.get_stable().await, 42);
    }

    #[tokio::test]
    async fn test_gradual_rollout_distribution() {
        let rollout = GradualRollout::new(Arc::new(42));
        rollout.set_canary(Arc::new(100), 50).await;

        // Test that roughly 50% get canary (with randomness)
        let mut canary_count = 0;
        let iterations = 1000;

        for _ in 0..iterations {
            let config = rollout.get(None).await;
            if *config != 42 {
                canary_count += 1;
            }
        }

        // Should be roughly 50/50 (allow 40-60% range due to randomness)
        let canary_percentage = (canary_count * 100) / iterations;
        assert!((40..=60).contains(&canary_percentage));
    }
}
