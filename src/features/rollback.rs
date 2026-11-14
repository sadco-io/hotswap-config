//! Configuration rollback support with version history.
//!
//! Tracks previous configuration versions and allows rolling back to earlier states.

use crate::core::HotswapConfig;
use crate::error::{ConfigError, Result};
use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, RwLock};

/// A versioned configuration snapshot.
pub struct ConfigVersion<T> {
    /// Version number (monotonically increasing)
    pub version: u64,
    /// Timestamp when this version was created
    pub timestamp: DateTime<Utc>,
    /// The configuration data
    pub config: Arc<T>,
    /// Optional description of why this version was created
    pub source: Option<String>,
}

impl<T> Clone for ConfigVersion<T> {
    fn clone(&self) -> Self {
        Self {
            version: self.version,
            timestamp: self.timestamp,
            config: Arc::clone(&self.config),
            source: self.source.clone(),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for ConfigVersion<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigVersion")
            .field("version", &self.version)
            .field("timestamp", &self.timestamp)
            .field("config", &*self.config)
            .field("source", &self.source)
            .finish()
    }
}

struct HistoryInner<T> {
    versions: VecDeque<ConfigVersion<T>>,
    max_size: usize,
    next_version: u64,
    /// Active position inside `versions` (0..versions.len() when non-empty).
    cursor: usize,
}

/// Configuration history tracker.
///
/// Maintains a bounded history of configuration versions and tracks the active
/// cursor so consecutive rollbacks walk backward cleanly instead of ping-ponging.
pub struct ConfigHistory<T> {
    inner: Arc<RwLock<HistoryInner<T>>>,
}

impl<T> Clone for ConfigHistory<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for ConfigHistory<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        f.debug_struct("ConfigHistory")
            .field("len", &inner.versions.len())
            .field("max_size", &inner.max_size)
            .field("cursor", &inner.cursor)
            .field("next_version", &inner.next_version)
            .field("versions", &inner.versions)
            .finish()
    }
}

impl<T> ConfigHistory<T> {
    /// Synchronously record a configuration snapshot.
    ///
    /// If the latest recorded entry already points to the exact same `Arc<T>` allocation
    /// (for instance when `config.update(x).await` auto-records `x` and the caller then
    /// calls `history.record(config.get(), Some("reason")).await` to attach a custom label),
    /// this updates that entry's `source` metadata in-place rather than pushing a duplicate.
    pub(crate) fn record_sync(&self, config: Arc<T>, source: Option<String>) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());

        if let Some(back) = inner.versions.back_mut() {
            if Arc::ptr_eq(&back.config, &config) {
                if source.is_some() {
                    back.source = source;
                }
                inner.cursor = inner.versions.len() - 1;
                return;
            }
        }

        let version_num = inner.next_version;
        inner.next_version += 1;

        inner.versions.push_back(ConfigVersion {
            version: version_num,
            timestamp: Utc::now(),
            config,
            source,
        });

        while inner.versions.len() > inner.max_size {
            inner.versions.pop_front();
        }

        inner.cursor = inner.versions.len() - 1;
    }

    pub(crate) fn prepare_rollback_steps(&self, steps: usize) -> Result<(usize, Arc<T>)> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if inner.versions.is_empty() {
            return Err(ConfigError::InsufficientHistory {
                requested: steps,
                available: 0,
            });
        }

        let available = inner.cursor;
        let target_idx =
            inner
                .cursor
                .checked_sub(steps)
                .ok_or(ConfigError::InsufficientHistory {
                    requested: steps,
                    available,
                })?;

        let cfg = Arc::clone(&inner.versions[target_idx].config);
        Ok((target_idx, cfg))
    }

    pub(crate) fn prepare_rollback_to_version(&self, version: u64) -> Result<(usize, Arc<T>)> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let (target_idx, entry) = inner
            .versions
            .iter()
            .enumerate()
            .find(|(_, v)| v.version == version)
            .ok_or_else(|| {
                ConfigError::Other(format!("Version {} not found in history", version))
            })?;

        Ok((target_idx, Arc::clone(&entry.config)))
    }

    pub(crate) fn set_cursor_sync(&self, target_idx: usize) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if target_idx < inner.versions.len() {
            inner.cursor = target_idx;
        }
    }
}

impl<T: Clone> ConfigHistory<T> {
    /// Create a new configuration history with a maximum size (`max_size >= 1`).
    ///
    /// # Arguments
    ///
    /// * `max_size` - Maximum number of versions to keep (older versions are dropped; clamped to at least `1`)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hotswap_config::features::ConfigHistory;
    ///
    /// let history: ConfigHistory<String> = ConfigHistory::new(10);
    /// ```
    pub fn new(max_size: usize) -> Self {
        let max_size = max_size.max(1);
        Self {
            inner: Arc::new(RwLock::new(HistoryInner {
                versions: VecDeque::with_capacity(max_size),
                max_size,
                next_version: 0,
                cursor: 0,
            })),
        }
    }

    /// Record a new configuration version.
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration to record
    /// * `source` - Optional description of the change source
    pub async fn record(&self, config: Arc<T>, source: Option<String>) {
        self.record_sync(config, source);
    }

    /// Get the currently active version number (or `0` if history is empty).
    pub async fn current_version(&self) -> u64 {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if inner.versions.is_empty() {
            inner.next_version.saturating_sub(1)
        } else {
            inner.versions[inner.cursor].version
        }
    }

    /// Get the total number of versions in history.
    pub async fn len(&self) -> usize {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.versions.len()
    }

    /// Check if the history is empty.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Get a specific version by version number.
    pub async fn get_version(&self, version: u64) -> Option<ConfigVersion<T>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .versions
            .iter()
            .find(|v| v.version == version)
            .cloned()
    }

    /// Get the N most recent versions.
    pub async fn get_recent(&self, count: usize) -> Vec<ConfigVersion<T>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.versions.iter().rev().take(count).cloned().collect()
    }

    /// Get all versions in chronological order.
    pub async fn get_all(&self) -> Vec<ConfigVersion<T>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.versions.iter().cloned().collect()
    }

    /// Look up the configuration snapshot for a specific version number without mutating the cursor.
    ///
    /// Returns `None` if the version is not in history.
    pub async fn rollback_to_version(&self, version: u64) -> Option<Arc<T>> {
        self.get_version(version).await.map(|v| v.config)
    }

    /// Look up the configuration snapshot `steps` back from the active cursor without mutating it.
    ///
    /// # Arguments
    ///
    /// * `steps` - Number of versions to step back (`1` = previous version)
    ///
    /// Returns `None` if stepping back that far exceeds available history.
    pub async fn rollback_steps(&self, steps: usize) -> Option<Arc<T>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if inner.versions.is_empty() {
            return None;
        }
        let index = inner.cursor.checked_sub(steps)?;
        inner.versions.get(index).map(|v| Arc::clone(&v.config))
    }
}

/// Extension trait for rollback support on [`HotswapConfig`].
pub trait Rollback<T> {
    /// Enable rollback support with a bounded history size.
    ///
    /// Synchronously records the current configuration as version `0` and attaches
    /// the returned [`ConfigHistory`] to the [`HotswapConfig`] so subsequent
    /// `reload()` and `update()` calls automatically record new versions.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use hotswap_config::features::Rollback;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, Clone)]
    /// struct AppConfig {
    ///     port: u16,
    /// }
    ///
    /// # async fn example(config: HotswapConfig<AppConfig>) -> Result<()> {
    /// let history = config.enable_history(10);
    ///
    /// // Make changes...
    /// config.reload().await?;
    ///
    /// // Rollback 1 step
    /// config.rollback(&history, 1).await?;
    /// # Ok(())
    /// # }
    /// ```
    fn enable_history(&self, max_size: usize) -> ConfigHistory<T>;

    /// Rollback `steps` versions backward from the active cursor.
    ///
    /// # Arguments
    ///
    /// * `history` - The [`ConfigHistory`] instance
    /// * `steps` - Number of versions to step back
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InsufficientHistory`] if `steps` exceeds available history.
    fn rollback(
        &self,
        history: &ConfigHistory<T>,
        steps: usize,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Rollback to a specific version number in `history`.
    fn rollback_to_version(
        &self,
        history: &ConfigHistory<T>,
        version: u64,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

impl<T> Rollback<T> for HotswapConfig<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn enable_history(&self, max_size: usize) -> ConfigHistory<T> {
        let _guard = self
            .inner
            .write_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let history = ConfigHistory::new(max_size);
        let current = self.get();
        history.record_sync(current, Some("Initial version".to_string()));

        *self.inner.history.lock().unwrap_or_else(|e| e.into_inner()) = Some(history.clone());

        history
    }

    async fn rollback(&self, history: &ConfigHistory<T>, steps: usize) -> Result<()> {
        {
            let _guard = self
                .inner
                .write_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());

            let (target_idx, target_config) = history.prepare_rollback_steps(steps)?;
            self.inner.validate_ref(&target_config)?;
            self.inner.current.store(Arc::clone(&target_config));
            history.set_cursor_sync(target_idx);

            #[cfg(feature = "metrics")]
            if let Some(metrics) = &self.inner.metrics {
                metrics.record_update();
            }
        }

        self.inner.notify_subscribers().await;
        Ok(())
    }

    async fn rollback_to_version(&self, history: &ConfigHistory<T>, version: u64) -> Result<()> {
        {
            let _guard = self
                .inner
                .write_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());

            let (target_idx, target_config) = history.prepare_rollback_to_version(version)?;
            self.inner.validate_ref(&target_config)?;
            self.inner.current.store(Arc::clone(&target_config));
            history.set_cursor_sync(target_idx);

            #[cfg(feature = "metrics")]
            if let Some(metrics) = &self.inner.metrics {
                metrics.record_update();
            }
        }

        self.inner.notify_subscribers().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_history_creation_and_debug() {
        let history: ConfigHistory<i32> = ConfigHistory::new(5);
        assert_eq!(history.len().await, 0);
        assert!(history.is_empty().await);
        assert!(format!("{:?}", history).contains("ConfigHistory"));
    }

    #[test]
    fn test_enable_history_works_synchronously_without_runtime() {
        let config = HotswapConfig::new(42);
        let history = config.enable_history(5);
        let dbg = format!("{:?}", history);
        assert!(dbg.contains("Initial version"));
    }

    #[tokio::test]
    async fn test_record_and_retrieve() {
        let history = ConfigHistory::new(5);

        history.record(Arc::new(1), Some("First".to_string())).await;
        history
            .record(Arc::new(2), Some("Second".to_string()))
            .await;

        assert_eq!(history.len().await, 2);

        let version = history.get_version(0).await.unwrap();
        assert_eq!(*version.config, 1);
        assert!(format!("{:?}", version).contains("ConfigVersion"));

        let version = history.get_version(1).await.unwrap();
        assert_eq!(*version.config, 2);
    }

    #[tokio::test]
    async fn test_max_size_limit_and_zero_clamping() {
        let zero_history = ConfigHistory::new(0);
        zero_history.record(Arc::new(99), None).await;
        assert_eq!(zero_history.len().await, 1);

        let history = ConfigHistory::new(3);
        for i in 0..5 {
            history.record(Arc::new(i), None).await;
        }

        assert_eq!(history.len().await, 3);
        assert!(history.get_version(0).await.is_none());
        assert!(history.get_version(1).await.is_none());
        assert!(history.get_version(2).await.is_some());
    }

    #[tokio::test]
    async fn test_rollback_steps() {
        let history = ConfigHistory::new(5);

        history.record(Arc::new(10), None).await;
        history.record(Arc::new(20), None).await;
        history.record(Arc::new(30), None).await;

        let config = history.rollback_steps(1).await.unwrap();
        assert_eq!(*config, 20);

        let config = history.rollback_steps(2).await.unwrap();
        assert_eq!(*config, 10);
    }

    #[tokio::test]
    async fn test_rollback_steps_exceeds_history() {
        let history = ConfigHistory::new(5);

        history.record(Arc::new(10), None).await;
        history.record(Arc::new(20), None).await;

        let config = history.rollback_steps(5).await;
        assert!(config.is_none());
    }

    #[tokio::test]
    async fn test_get_recent() {
        let history = ConfigHistory::new(10);

        for i in 0..5 {
            history.record(Arc::new(i), None).await;
        }

        let recent = history.get_recent(3).await;
        assert_eq!(recent.len(), 3);
        assert_eq!(*recent[0].config, 4);
        assert_eq!(*recent[1].config, 3);
        assert_eq!(*recent[2].config, 2);
    }

    #[tokio::test]
    async fn test_current_version() {
        let history = ConfigHistory::new(5);

        history.record(Arc::new(1), None).await;
        assert_eq!(history.current_version().await, 0);

        history.record(Arc::new(2), None).await;
        assert_eq!(history.current_version().await, 1);
    }

    #[tokio::test]
    async fn test_consecutive_rollback_no_ping_pong_and_auto_record() {
        let config = HotswapConfig::new(10);
        let history = config.enable_history(10);

        // Initial version recorded synchronously with zero sleep!
        assert_eq!(history.len().await, 1);

        // Auto-recorded on update()!
        config.update(20).await.unwrap();
        // Calling manual record() with the same Arc updates description without duplicating!
        history
            .record(config.get(), Some("Update to 20".to_string()))
            .await;
        assert_eq!(history.len().await, 2);

        config.update(30).await.unwrap();
        assert_eq!(history.len().await, 3);
        assert_eq!(*config.get(), 30);

        // Consecutive rollback(1) walks backward 30 -> 20 -> 10 without ping-ponging!
        config.rollback(&history, 1).await.unwrap();
        assert_eq!(*config.get(), 20);

        config.rollback(&history, 1).await.unwrap();
        assert_eq!(*config.get(), 10);

        // One more step exceeds available history and returns InsufficientHistory!
        let err = config.rollback(&history, 1).await.unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InsufficientHistory {
                requested: 1,
                available: 0
            }
        ));

        // Can jump forward to version 2 (value 30) via rollback_to_version!
        config.rollback_to_version(&history, 2).await.unwrap();
        assert_eq!(*config.get(), 30);
    }
}
