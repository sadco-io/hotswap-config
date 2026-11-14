//! The main configuration handle providing lock-free access.

use crate::core::ConfigLoader;
use crate::error::{ConfigError, Result, ValidationError};
use arc_swap::ArcSwap;
use serde::de::DeserializeOwned;
use std::fmt;
use std::sync::{Arc, Mutex};

#[cfg(feature = "file-watch")]
use crate::notify::{ConfigWatcher, SubscriberRegistry};

#[cfg(feature = "metrics")]
use crate::metrics::ConfigMetrics;

/// Type alias for validator functions.
pub(crate) type Validator<T> =
    Arc<dyn Fn(&T) -> std::result::Result<(), ValidationError> + Send + Sync>;

/// Shared internal state of `HotswapConfig<T>`.
///
/// Kept separate from `watcher` so background file-watching tasks can hold a
/// `Weak<HotswapConfigInner<T>>` without keeping `ConfigWatcher` (and thus
/// themselves) alive after all user `HotswapConfig<T>` handles are dropped.
pub(crate) struct HotswapConfigInner<T> {
    /// The current configuration, stored inline in `ArcSwap` for single-indirection reads
    pub(crate) current: ArcSwap<T>,
    /// Configuration loader for reloading from sources
    pub(crate) loader: Option<Arc<ConfigLoader>>,
    /// Optional validator function
    pub(crate) validator: Option<Validator<T>>,
    /// Serializes concurrent reload I/O operations across `.await` when tokio is enabled
    #[cfg(feature = "tokio")]
    pub(crate) reload_lock: tokio::sync::Mutex<()>,
    /// Serializes concurrent write/reload/patch/rollback commits
    pub(crate) write_lock: Mutex<()>,
    /// Subscriber registry for change notifications
    #[cfg(feature = "file-watch")]
    pub(crate) subscribers: SubscriberRegistry,
    /// Optional metrics collector
    #[cfg(feature = "metrics")]
    pub(crate) metrics: Option<ConfigMetrics>,
    /// Optional attached version history for automatic recording on reload/update
    #[cfg(feature = "rollback")]
    pub(crate) history: Mutex<Option<crate::features::ConfigHistory<T>>>,
}

impl<T> HotswapConfigInner<T> {
    /// Validate a candidate configuration against the registered validator, if any.
    pub(crate) fn validate_ref(&self, candidate: &T) -> Result<()> {
        if let Some(validator) = &self.validator {
            if let Err(err) = validator(candidate) {
                #[cfg(feature = "metrics")]
                if let Some(metrics) = &self.metrics {
                    metrics.record_validation_failure();
                }
                return Err(ConfigError::ValidationError(err));
            }
        }
        Ok(())
    }

    /// Notify all subscribers if `file-watch` is enabled.
    pub(crate) async fn notify_subscribers(&self) {
        #[cfg(feature = "file-watch")]
        self.subscribers.notify_all().await;
    }

    /// Perform a full reload from `loader`, offloading blocking I/O onto `spawn_blocking`
    /// when running inside a Tokio runtime so `CurrentThread` runtimes never stall.
    pub(crate) async fn reload(&self) -> Result<()>
    where
        T: DeserializeOwned,
    {
        #[cfg(feature = "metrics")]
        let timer = self.metrics.as_ref().map(|m| m.start_reload());

        #[cfg(feature = "tokio")]
        let _reload_guard = self.reload_lock.lock().await;

        let result = async {
            let loader = self
                .loader
                .as_ref()
                .ok_or_else(|| ConfigError::Other("No loader available for reload".to_string()))?;

            #[cfg(feature = "tokio")]
            let merged = if tokio::runtime::Handle::try_current().is_ok() {
                let loader_arc = Arc::clone(loader);
                tokio::task::spawn_blocking(move || loader_arc.collect_merged())
                    .await
                    .map_err(|e| ConfigError::LoadError(format!("Reload task failed: {}", e)))??
            } else {
                loader.collect_merged()?
            };

            #[cfg(not(feature = "tokio"))]
            let merged = loader.collect_merged()?;

            let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let new_config: T = ConfigLoader::deserialize_merged(merged)?;
            self.validate_ref(&new_config)?;

            let arc_config = Arc::new(new_config);
            self.current.store(Arc::clone(&arc_config));

            #[cfg(feature = "rollback")]
            if let Some(history) = self
                .history
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                history.record_sync(arc_config, Some("Reload".to_string()));
            }

            Ok(())
        }
        .await;

        #[cfg(feature = "metrics")]
        if let (Some(metrics), Some(start)) = (&self.metrics, timer) {
            match &result {
                Ok(()) => metrics.record_reload_success(start),
                Err(_) => metrics.record_reload_failure(start),
            }
        }

        if result.is_ok() {
            self.notify_subscribers().await;
        }

        result
    }
}

/// The main configuration handle providing lock-free reads and atomic updates.
///
/// This is the primary interface for accessing configuration. It uses `arc-swap`
/// internally to provide zero-cost reads while allowing atomic updates.
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::prelude::*;
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize, Clone)]
/// struct AppConfig {
///     port: u16,
/// }
///
/// # async fn example() -> Result<()> {
/// let config = HotswapConfig::builder()
///     .with_file("config.yaml")
///     .build::<AppConfig>()
///     .await?;
///
/// // Zero-cost read
/// let cfg = config.get();
/// println!("Port: {}", cfg.port);
/// # Ok(())
/// # }
/// ```
pub struct HotswapConfig<T> {
    pub(crate) inner: Arc<HotswapConfigInner<T>>,
    /// Optional file watcher tied to the lifetime of user `HotswapConfig` handles
    #[cfg(feature = "file-watch")]
    pub(crate) watcher: Option<Arc<ConfigWatcher>>,
}

impl<T: fmt::Debug> fmt::Debug for HotswapConfig<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let current = self.get();
        let mut s = f.debug_struct("HotswapConfig");
        s.field("current", &*current)
            .field("has_loader", &self.inner.loader.is_some())
            .field("has_validator", &self.inner.validator.is_some());
        #[cfg(feature = "file-watch")]
        s.field("is_watching", &self.watcher.is_some());
        #[cfg(feature = "metrics")]
        s.field("has_metrics", &self.inner.metrics.is_some());
        s.finish()
    }
}

impl<T> HotswapConfig<T> {
    /// Create a new configuration handle with an initial value.
    ///
    /// This creates a basic configuration handle without any loading or validation.
    /// For most use cases, prefer using `HotswapConfig::builder()` instead.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hotswap_config::prelude::*;
    ///
    /// let config = HotswapConfig::new(42);
    /// assert_eq!(*config.get(), 42);
    /// ```
    pub fn new(initial: T) -> Self {
        Self {
            inner: Arc::new(HotswapConfigInner {
                current: ArcSwap::new(Arc::new(initial)),
                loader: None,
                validator: None,
                #[cfg(feature = "tokio")]
                reload_lock: tokio::sync::Mutex::new(()),
                write_lock: Mutex::new(()),
                #[cfg(feature = "file-watch")]
                subscribers: SubscriberRegistry::new(),
                #[cfg(feature = "metrics")]
                metrics: None,
                #[cfg(feature = "rollback")]
                history: Mutex::new(None),
            }),
            #[cfg(feature = "file-watch")]
            watcher: None,
        }
    }

    /// Create a configuration handle with loader and validator support.
    pub(crate) fn with_loader(
        initial: T,
        loader: Arc<ConfigLoader>,
        validator: Option<Validator<T>>,
        #[cfg(feature = "metrics")] meter: Option<opentelemetry::metrics::Meter>,
    ) -> Self {
        #[cfg(feature = "metrics")]
        let metrics = meter.map(ConfigMetrics::new);

        #[cfg(feature = "file-watch")]
        let subscribers = {
            #[cfg(feature = "metrics")]
            {
                if let Some(ref m) = metrics {
                    let m_clone = m.clone();
                    SubscriberRegistry::with_on_count_change(move |count| {
                        m_clone.update_subscriber_count(count as i64);
                    })
                } else {
                    SubscriberRegistry::new()
                }
            }
            #[cfg(not(feature = "metrics"))]
            {
                SubscriberRegistry::new()
            }
        };

        Self {
            inner: Arc::new(HotswapConfigInner {
                current: ArcSwap::new(Arc::new(initial)),
                loader: Some(loader),
                validator,
                #[cfg(feature = "tokio")]
                reload_lock: tokio::sync::Mutex::new(()),
                write_lock: Mutex::new(()),
                #[cfg(feature = "file-watch")]
                subscribers,
                #[cfg(feature = "metrics")]
                metrics,
                #[cfg(feature = "rollback")]
                history: Mutex::new(None),
            }),
            #[cfg(feature = "file-watch")]
            watcher: None,
        }
    }

    /// Set the file watcher for this configuration.
    #[cfg(feature = "file-watch")]
    pub(crate) fn with_watcher(mut self, watcher: Arc<ConfigWatcher>) -> Self {
        self.watcher = Some(watcher);
        self
    }

    /// Get a reference-counted handle to the current configuration.
    ///
    /// This is a zero-cost operation that returns an `Arc<T>`. Readers never
    /// block writers or other readers.
    ///
    /// # Performance
    ///
    /// This operation is lock-free and typically completes in < 10 nanoseconds.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hotswap_config::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Debug, Deserialize, Clone)]
    /// # struct AppConfig { port: u16 }
    /// # async fn example(config: HotswapConfig<AppConfig>) {
    /// let cfg = config.get();
    /// println!("Port: {}", cfg.port);
    /// # }
    /// ```
    pub fn get(&self) -> Arc<T> {
        self.inner.current.load_full()
    }

    /// Manually reload configuration from all sources.
    ///
    /// This triggers a full reload, respecting the precedence order.
    /// If validation fails, the old configuration is retained.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No loader is available (e.g. constructed via `HotswapConfig::new`)
    /// - Configuration sources cannot be read
    /// - Deserialization fails
    /// - Validation fails (if a validator is configured)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hotswap_config::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Debug, Deserialize, Clone)]
    /// # struct AppConfig { port: u16 }
    /// # async fn example(config: HotswapConfig<AppConfig>) -> Result<()> {
    /// // Manually trigger a reload
    /// config.reload().await?;
    ///
    /// let cfg = config.get();
    /// println!("Reloaded config, port: {}", cfg.port);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reload(&self) -> Result<()>
    where
        T: DeserializeOwned + Clone,
    {
        self.inner.reload().await
    }

    /// Update configuration with a new value directly.
    ///
    /// This bypasses the loader and directly updates the configuration.
    /// Useful for programmatic updates or testing.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::ValidationError` if validation fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hotswap_config::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Debug, Deserialize, Clone)]
    /// # struct AppConfig { port: u16 }
    /// # async fn example(config: HotswapConfig<AppConfig>) -> Result<()> {
    /// let new_config = AppConfig { port: 9090 };
    /// config.update(new_config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update(&self, new_config: T) -> Result<()> {
        {
            let _guard = self
                .inner
                .write_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.inner.validate_ref(&new_config)?;
            let arc_config = Arc::new(new_config);
            self.inner.current.store(Arc::clone(&arc_config));

            #[cfg(feature = "rollback")]
            if let Some(history) = self
                .inner
                .history
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                history.record_sync(arc_config, Some("Update".to_string()));
            }

            #[cfg(feature = "metrics")]
            if let Some(metrics) = &self.inner.metrics {
                metrics.record_update();
            }
        }

        self.inner.notify_subscribers().await;
        Ok(())
    }

    /// Subscribe to configuration changes.
    ///
    /// The provided callback will be invoked whenever the configuration
    /// is reloaded or updated. Returns a handle that can be dropped to unsubscribe.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hotswap_config::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Debug, Deserialize, Clone)]
    /// # struct AppConfig { port: u16 }
    /// # async fn example(config: HotswapConfig<AppConfig>) {
    /// let handle = config.subscribe(|| {
    ///     println!("Configuration changed!");
    /// }).await;
    ///
    /// // Later, unsubscribe
    /// drop(handle);
    /// # }
    /// ```
    #[cfg(feature = "file-watch")]
    pub async fn subscribe<F>(&self, callback: F) -> crate::notify::SubscriptionHandle
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.subscribers.subscribe(callback).await
    }

    /// Returns `true` if automatic file watching is active for this configuration handle.
    ///
    /// File watching is configured via
    /// [`HotswapConfigBuilder::with_file_watch`](crate::core::HotswapConfigBuilder::with_file_watch).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hotswap_config::prelude::*;
    /// # use serde::Deserialize;
    /// # #[derive(Debug, Deserialize, Clone)]
    /// # struct AppConfig { port: u16 }
    /// # async fn example() -> Result<()> {
    /// let config = HotswapConfig::builder()
    ///     .with_file("config.yaml")
    ///     .with_file_watch(true)
    ///     .build::<AppConfig>()
    ///     .await?;
    ///
    /// assert!(config.is_watching());
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "file-watch")]
    pub fn is_watching(&self) -> bool {
        self.watcher.is_some()
    }

    /// Get a reference to the [`ConfigMetrics`] collector, if metrics were enabled via
    /// [`HotswapConfigBuilder::with_metrics`](crate::core::HotswapConfigBuilder::with_metrics).
    #[cfg(feature = "metrics")]
    pub fn metrics(&self) -> Option<&ConfigMetrics> {
        self.inner.metrics.as_ref()
    }
}

impl<T> Clone for HotswapConfig<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            #[cfg(feature = "file-watch")]
            watcher: self.watcher.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct TestConfig {
        value: i32,
    }

    #[test]
    fn test_create_and_read() {
        let config = HotswapConfig::new(TestConfig { value: 42 });
        let cfg = config.get();
        assert_eq!(cfg.value, 42);
        assert!(format!("{:?}", config).contains("HotswapConfig"));
    }

    #[test]
    fn test_clone() {
        let config = HotswapConfig::new(TestConfig { value: 42 });
        let config2 = config.clone();

        let cfg1 = config.get();
        let cfg2 = config2.get();

        assert_eq!(cfg1.value, cfg2.value);
    }
}
