//! Builder for constructing HotswapConfig instances.

use crate::core::{ConfigLoader, HotswapConfig};
use crate::error::{ConfigError, Result, ValidationError};
use crate::sources::{ConfigSource, EnvSource, FileSource};
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "metrics")]
use opentelemetry::metrics::Meter;

#[cfg(feature = "validation")]
use crate::core::Validate;

#[cfg(feature = "file-watch")]
use crate::notify::ConfigWatcher;
#[cfg(feature = "file-watch")]
use std::time::Duration;

/// Type alias for any-based validator functions used during building.
type AnyValidator =
    Arc<dyn Fn(&dyn std::any::Any) -> std::result::Result<(), ValidationError> + Send + Sync>;

/// Type alias for typed validator functions.
type TypedValidator<T> = Arc<dyn Fn(&T) -> std::result::Result<(), ValidationError> + Send + Sync>;

/// Builder for constructing a `HotswapConfig` instance.
///
/// Provides a fluent interface for configuring all aspects of configuration loading.
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
///     .with_file("config/default.yaml")
///     .with_file("config/production.yaml")
///     .with_env_overrides("APP", "__")
///     .build::<AppConfig>()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct HotswapConfigBuilder {
    file_paths: Vec<PathBuf>,
    env_prefix: Option<String>,
    env_separator: Option<String>,
    custom_sources: Vec<Box<dyn ConfigSource>>,
    validator: Option<AnyValidator>,
    #[cfg(feature = "file-watch")]
    enable_file_watch: bool,
    #[cfg(feature = "file-watch")]
    watch_debounce: Duration,
    #[cfg(feature = "metrics")]
    meter: Option<Meter>,
}

impl HotswapConfigBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            file_paths: Vec::new(),
            env_prefix: None,
            env_separator: None,
            custom_sources: Vec::new(),
            validator: None,
            #[cfg(feature = "file-watch")]
            enable_file_watch: false,
            #[cfg(feature = "file-watch")]
            watch_debounce: Duration::from_millis(500),
            #[cfg(feature = "metrics")]
            meter: None,
        }
    }

    /// Add a file source with automatic format detection.
    ///
    /// Supported formats: YAML (.yaml, .yml), TOML (.toml), JSON (.json)
    ///
    /// Files are added in the order they are specified. Later files have higher
    /// priority and will override earlier files.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    ///
    /// # async fn example() {
    /// HotswapConfig::builder()
    ///     .with_file("config/default.yaml")
    ///     .with_file("config/production.yaml");
    /// # }
    /// ```
    pub fn with_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file_paths.push(path.into());
        self
    }

    /// Add environment variable source with custom prefix.
    ///
    /// # Arguments
    ///
    /// * `prefix` - Prefix for environment variables (e.g., "APP")
    /// * `separator` - Separator for nested keys (e.g., "__" for APP_DB__HOST)
    ///
    /// Environment variables have the highest priority by default (300).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    ///
    /// # async fn example() {
    /// // APP_SERVER__PORT=8080 -> server.port = 8080
    /// HotswapConfig::builder()
    ///     .with_env_overrides("APP", "__");
    /// # }
    /// ```
    pub fn with_env_overrides(mut self, prefix: &str, separator: &str) -> Self {
        self.env_prefix = Some(prefix.to_string());
        self.env_separator = Some(separator.to_string());
        self
    }

    /// Add a custom configuration source.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use hotswap_config::sources::FileSource;
    ///
    /// # async fn example() {
    /// let custom_source = FileSource::new("config/custom.yaml")
    ///     .with_priority(150);
    ///
    /// HotswapConfig::builder()
    ///     .with_source(custom_source);
    /// # }
    /// ```
    pub fn with_source<S: ConfigSource + 'static>(mut self, source: S) -> Self {
        self.custom_sources.push(Box::new(source));
        self
    }

    /// Add a validation function that must pass before the config is loaded.
    ///
    /// The validator is called during the initial build and before every reload.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use hotswap_config::error::ValidationError;
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
    ///     .with_validation(|config: &AppConfig| {
    ///         if config.port < 1024 {
    ///             return Err(ValidationError::invalid_field(
    ///                 "port",
    ///                 "must be >= 1024"
    ///             ));
    ///         }
    ///         Ok(())
    ///     })
    ///     .build::<AppConfig>()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_validation<F, T>(mut self, validator: F) -> Self
    where
        F: Fn(&T) -> std::result::Result<(), ValidationError> + Send + Sync + 'static,
        T: 'static,
    {
        self.validator = Some(Arc::new(move |config: &dyn std::any::Any| {
            let typed_config = config
                .downcast_ref::<T>()
                .ok_or_else(|| ValidationError::custom("Type mismatch in validator"))?;
            validator(typed_config)
        }));
        self
    }

    /// Enable file watching for automatic reloads.
    ///
    /// When enabled, the configuration will automatically reload when any
    /// watched file changes. Uses a default debounce of 500ms.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    ///
    /// # async fn example() {
    /// HotswapConfig::builder()
    ///     .with_file("config.yaml")
    ///     .with_file_watch(true);
    /// # }
    /// ```
    #[cfg(feature = "file-watch")]
    pub fn with_file_watch(mut self, enabled: bool) -> Self {
        self.enable_file_watch = enabled;
        self
    }

    /// Set the debounce duration for file watching.
    ///
    /// This is the minimum time between reload triggers when files change rapidly.
    /// Default is 500ms.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// HotswapConfig::builder()
    ///     .with_file("config.yaml")
    ///     .with_file_watch(true)
    ///     .with_watch_debounce(Duration::from_secs(1));
    /// # }
    /// ```
    #[cfg(feature = "file-watch")]
    pub fn with_watch_debounce(mut self, duration: Duration) -> Self {
        self.watch_debounce = duration;
        self
    }

    /// Enable metrics collection with the provided meter.
    ///
    /// When enabled, the configuration will track reload attempts, success/failure
    /// rates, latencies, and subscriber counts using OpenTelemetry metrics.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use opentelemetry::global;
    ///
    /// # async fn example() {
    /// let meter = global::meter("my-app");
    ///
    /// HotswapConfig::builder()
    ///     .with_file("config.yaml")
    ///     .with_metrics(meter);
    /// # }
    /// ```
    #[cfg(feature = "metrics")]
    pub fn with_metrics(mut self, meter: Meter) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Build the configuration handle.
    ///
    /// This performs the initial load from all sources and validates the result.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The configuration type (must implement `DeserializeOwned`)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Initial configuration load fails
    /// - Deserialization fails
    /// - Validation fails
    pub async fn build<T>(self) -> Result<HotswapConfig<T>>
    where
        T: DeserializeOwned + Clone + Send + Sync + 'static,
    {
        let mut loader = ConfigLoader::new();

        // Add file sources with increasing priority
        for (index, path) in self.file_paths.iter().enumerate() {
            let priority = 100 + (index as i32 * 10); // 100, 110, 120, etc.
            let source = FileSource::new(path).with_priority(priority);
            loader.add_source(Box::new(source));
        }

        // Add custom sources
        for source in self.custom_sources {
            loader.add_source(source);
        }

        // Add environment variable source (highest priority)
        if let (Some(prefix), Some(separator)) = (self.env_prefix, self.env_separator) {
            let env_source = EnvSource::new(prefix, separator);
            loader.add_source(Box::new(env_source));
        }

        // Load the configuration
        let config: T = loader.load()?;

        // Convert the Any-based validator to a typed validator
        let typed_validator: Option<TypedValidator<T>> = self.validator.as_ref().map(|v| {
            let validator = Arc::clone(v);
            Arc::new(move |config: &T| validator(config as &dyn std::any::Any)) as TypedValidator<T>
        });

        // Validate if a validator was provided
        if let Some(validator) = &typed_validator {
            validator(&config).map_err(|e| ConfigError::ValidationError(e.to_string()))?;
        }

        // Also validate using Validate trait if feature is enabled
        #[cfg(feature = "validation")]
        if let Some(validatable) = (&config as &dyn std::any::Any).downcast_ref::<&dyn Validate>() {
            validatable
                .validate()
                .map_err(|e| ConfigError::ValidationError(e.to_string()))?;
        }

        // Create the config handle with loader, validator, and metrics
        #[cfg(feature = "file-watch")]
        let mut hotswap_config = HotswapConfig::with_loader(
            config,
            loader,
            typed_validator,
            #[cfg(feature = "metrics")]
            self.meter,
        );
        #[cfg(not(feature = "file-watch"))]
        let hotswap_config = HotswapConfig::with_loader(
            config,
            loader,
            typed_validator,
            #[cfg(feature = "metrics")]
            self.meter,
        );

        // Set up file watching if enabled
        #[cfg(feature = "file-watch")]
        if self.enable_file_watch {
            let (watcher, mut rx) = ConfigWatcher::new(self.watch_debounce)
                .map_err(|e| ConfigError::Other(format!("Failed to create file watcher: {}", e)))?;

            // Watch all file paths
            for path in &self.file_paths {
                watcher.watch(path).await?;
            }

            let watcher_arc = Arc::new(watcher);
            hotswap_config = hotswap_config.with_watcher(Arc::clone(&watcher_arc));

            // Spawn a task to handle reload signals
            let config_clone = hotswap_config.clone();
            tokio::spawn(async move {
                while let Some(()) = rx.recv().await {
                    if let Err(e) = config_clone.reload().await {
                        eprintln!("Auto-reload failed: {}", e);
                    }
                }
            });
        }

        Ok(hotswap_config)
    }
}

impl Default for HotswapConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HotswapConfig<()> {
    /// Create a new builder for constructing a configuration handle.
    pub fn builder() -> HotswapConfigBuilder {
        HotswapConfigBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, Clone, PartialEq)]
    struct TestConfig {
        port: u16,
        host: String,
    }

    #[tokio::test]
    async fn test_builder_with_validation() {
        let builder = HotswapConfigBuilder::new().with_validation(|config: &TestConfig| {
            if config.port < 1024 {
                return Err(ValidationError::invalid_field("port", "must be >= 1024"));
            }
            Ok(())
        });

        // Should be able to build (validation happens in build())
        assert!(builder.file_paths.is_empty());
    }

    #[test]
    fn test_builder_accumulates_files() {
        let builder = HotswapConfigBuilder::new()
            .with_file("config1.yaml")
            .with_file("config2.yaml")
            .with_file("config3.yaml");

        assert_eq!(builder.file_paths.len(), 3);
    }

    #[test]
    fn test_builder_env_overrides() {
        let builder = HotswapConfigBuilder::new().with_env_overrides("APP", "__");

        assert_eq!(builder.env_prefix, Some("APP".to_string()));
        assert_eq!(builder.env_separator, Some("__".to_string()));
    }
}
