# hotswap-config: Zero-Downtime Configuration Library

> **Note:** This is the original pre-v0.1 architecture design document preserved for historical context. Several module paths, trait signatures, and planned integrations evolved prior to implementation; see [`README.md`](README.md) and [docs.rs/hotswap-config](https://docs.rs/hotswap-config) for the current v0.3 API.

## Overview

`hotswap-config` is a Rust library for managing application configuration with zero-downtime hot-reloads, atomic updates, and production-grade reliability patterns. It combines the battle-tested precedence model from aegis-gateway with lock-free atomic swapping using `arc-swap` and immutable data structures.

## Core Design Principles

1. **Zero-downtime updates**: Configuration changes never block readers
2. **Type-safe**: Leverage Rust's type system for compile-time safety
3. **Atomic consistency**: Readers always see a complete, valid configuration
4. **Composable**: Mix and match features based on needs
5. **Observable**: Built-in metrics and change notifications
6. **Gradual adoption**: Works with existing `config` crate patterns

## Crate Architecture

```
hotswap-config/
├── Cargo.toml
├── src/
│   ├── lib.rs                      # Public API and re-exports
│   ├── core/
│   │   ├── mod.rs                  # Core module exports
│   │   ├── config_handle.rs        # Arc<Config> wrapper with arc-swap
│   │   ├── builder.rs              # Builder pattern for setup
│   │   ├── loader.rs               # Load from files/env/sources
│   │   ├── precedence.rs           # Precedence resolution logic
│   │   └── validation.rs           # Validation trait and helpers
│   ├── sources/
│   │   ├── mod.rs                  # Configuration source trait
│   │   ├── file.rs                 # File-based source (YAML/TOML/JSON)
│   │   ├── env.rs                  # Environment variable source
│   │   ├── watch.rs                # File watching with notify crate
│   │   └── remote.rs               # Remote sources (feature-gated)
│   ├── features/
│   │   ├── mod.rs                  # Optional feature modules
│   │   ├── partial_update.rs       # Partial config updates
│   │   ├── rollback.rs             # Config history and rollback
│   │   ├── gradual_rollout.rs      # A/B testing and gradual deploy
│   │   └── secrets.rs              # Secret management integration
│   ├── notify/
│   │   ├── mod.rs                  # Change notification system
│   │   ├── subscriber.rs           # Subscriber trait and registry
│   │   └── channel.rs              # Async notification channels
│   ├── error.rs                    # Error types
│   ├── metrics.rs                  # Internal metrics collection
│   └── prelude.rs                  # Convenience re-exports
├── examples/
│   ├── basic_usage.rs
│   ├── hot_reload.rs
│   ├── validation.rs
│   ├── partial_updates.rs
│   ├── rollback.rs
│   └── gradual_rollout.rs
└── tests/
    ├── integration/
    │   ├── reload_tests.rs
    │   ├── precedence_tests.rs
    │   └── concurrent_tests.rs
    └── unit/
```

## Feature Flags

```toml
[features]
default = ["file-watch", "validation"]

# Core features
file-watch = ["notify"]              # Automatic file watching
validation = []                       # Config validation support

# File format support
yaml = ["serde_yaml"]
toml = ["toml"]
json = ["serde_json"]
all-formats = ["yaml", "toml", "json"]

# Advanced features
partial-updates = ["json-patch"]      # Partial config updates
rollback = []                         # Config history and rollback
gradual-rollout = ["fastrand"]        # A/B testing and gradual deploy
remote = ["reqwest", "async-trait"]   # Remote config sources

# Secret management
secrets-vault = ["vaultrs"]           # HashiCorp Vault
secrets-aws = ["aws-sdk-secretsmanager"]  # AWS Secrets Manager
secrets-gcp = ["google-secretmanager"] # GCP Secret Manager

# Observability
metrics = ["opentelemetry"]           # Built-in metrics
tracing = ["tracing"]                 # Structured logging

# Async runtime support
tokio-runtime = ["tokio"]
async-std-runtime = ["async-std"]

# Serialization
serde = ["serde/derive"]
```

## Core API Design

### 1. Main Config Handle

```rust
use arc_swap::ArcSwap;
use std::sync::Arc;

/// The main configuration handle providing lock-free reads and atomic updates.
///
/// This is the primary interface for accessing configuration. It uses `arc-swap`
/// internally to provide zero-cost reads while allowing atomic updates.
pub struct HotswapConfig<T> {
    /// The current configuration, wrapped in ArcSwap for atomic updates
    current: Arc<ArcSwap<T>>,

    /// Configuration loader for reloading
    loader: Arc<ConfigLoader>,

    /// Optional file watcher for automatic reloads
    #[cfg(feature = "file-watch")]
    watcher: Option<ConfigWatcher>,

    /// Validation function called before applying updates
    validator: Option<Arc<dyn Fn(&T) -> Result<(), ValidationError> + Send + Sync>>,

    /// Subscribers to notify on config changes
    subscribers: Arc<SubscriberRegistry<T>>,

    /// Optional history for rollback support
    #[cfg(feature = "rollback")]
    history: Arc<ConfigHistory<T>>,

    /// Metrics collector
    #[cfg(feature = "metrics")]
    metrics: Arc<ConfigMetrics>,
}

impl<T: Clone + Send + Sync + 'static> HotswapConfig<T> {
    /// Get a reference-counted handle to the current configuration.
    ///
    /// This is a zero-cost operation that returns an `Arc<T>`. Readers never
    /// block writers or other readers.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let config_handle = HotswapConfig::builder()
    ///     .with_file("config/default.yaml")
    ///     .build::<AppConfig>()?;
    ///
    /// // Zero-cost read
    /// let config = config_handle.get();
    /// println!("Server port: {}", config.server.port);
    /// ```
    pub fn get(&self) -> Arc<T> {
        self.current.load_full()
    }

    /// Manually reload configuration from all sources.
    ///
    /// This triggers a full reload, respecting the precedence order.
    /// If validation fails, the old configuration is retained.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Configuration sources cannot be read
    /// - Deserialization fails
    /// - Validation fails (if a validator is configured)
    pub async fn reload(&self) -> Result<(), ConfigError> {
        // Implementation
    }

    /// Subscribe to configuration changes.
    ///
    /// The callback will be invoked whenever the configuration is successfully updated.
    ///
    /// # Examples
    ///
    /// ```rust
    /// config_handle.subscribe(|new_config| {
    ///     info!("Config updated: rate_limit={}", new_config.rate_limit);
    /// });
    /// ```
    pub fn subscribe<F>(&self, callback: F) -> SubscriptionHandle
    where
        F: Fn(Arc<T>) + Send + Sync + 'static,
    {
        // Implementation
    }

    /// Update configuration with a new value.
    ///
    /// This bypasses the loader and directly updates the configuration.
    /// Useful for programmatic updates or testing.
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails.
    pub async fn update(&self, new_config: T) -> Result<(), ConfigError> {
        // Implementation
    }

    /// Rollback to a previous configuration version.
    ///
    /// # Arguments
    ///
    /// * `steps` - Number of versions to roll back (1 = previous version)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Rollback history is not available (feature not enabled)
    /// - Not enough history exists
    #[cfg(feature = "rollback")]
    pub async fn rollback(&self, steps: usize) -> Result<(), ConfigError> {
        // Implementation
    }

    /// Get the current configuration version.
    #[cfg(feature = "rollback")]
    pub fn version(&self) -> u64 {
        // Implementation
    }

    /// Get configuration history.
    #[cfg(feature = "rollback")]
    pub fn history(&self) -> Vec<ConfigVersion<T>> {
        // Implementation
    }
}
```

### 2. Builder Pattern

```rust
/// Builder for constructing a `HotswapConfig` instance.
///
/// Provides a fluent interface for configuring all aspects of configuration loading,
/// validation, and hot-reload behavior.
pub struct HotswapConfigBuilder {
    sources: Vec<Box<dyn ConfigSource>>,
    env_prefix: Option<String>,
    env_separator: Option<String>,
    file_watch_enabled: bool,
    watch_debounce_ms: u64,
    history_size: usize,
    validation_on_load: bool,
    metrics_enabled: bool,
}

impl HotswapConfigBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            env_prefix: Some("APP".to_string()),
            env_separator: Some("__".to_string()),
            file_watch_enabled: true,
            watch_debounce_ms: 500,
            history_size: 10,
            validation_on_load: true,
            metrics_enabled: true,
        }
    }

    /// Add a file source with automatic format detection.
    ///
    /// Supported formats: YAML (.yaml, .yml), TOML (.toml), JSON (.json)
    ///
    /// # Examples
    ///
    /// ```rust
    /// HotswapConfig::builder()
    ///     .with_file("config/default.yaml")
    ///     .with_file("config/production.yaml")
    ///     .build::<AppConfig>()?;
    /// ```
    pub fn with_file<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.sources.push(Box::new(FileSource::new(path.into())));
        self
    }

    /// Add environment variable source with custom prefix.
    ///
    /// # Arguments
    ///
    /// * `prefix` - Prefix for environment variables (e.g., "APP")
    /// * `separator` - Separator for nested keys (e.g., "__" for APP_DB__HOST)
    ///
    /// # Examples
    ///
    /// ```rust
    /// HotswapConfig::builder()
    ///     .with_env_overrides("APP", "__")
    ///     .build::<AppConfig>()?;
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
    /// ```rust
    /// let remote_source = HttpConfigSource::new("https://config-server/api/config");
    ///
    /// HotswapConfig::builder()
    ///     .with_source(remote_source)
    ///     .build::<AppConfig>()?;
    /// ```
    pub fn with_source<S: ConfigSource + 'static>(mut self, source: S) -> Self {
        self.sources.push(Box::new(source));
        self
    }

    /// Enable or disable file watching for automatic reloads.
    ///
    /// When enabled, configuration files are monitored for changes and
    /// automatically reloaded.
    #[cfg(feature = "file-watch")]
    pub fn with_file_watch(mut self, enabled: bool) -> Self {
        self.file_watch_enabled = enabled;
        self
    }

    /// Set the debounce delay for file watch events (in milliseconds).
    ///
    /// This prevents rapid reloads when multiple files change simultaneously.
    #[cfg(feature = "file-watch")]
    pub fn with_watch_debounce(mut self, ms: u64) -> Self {
        self.watch_debounce_ms = ms;
        self
    }

    /// Add a validation function that must pass before updates are applied.
    ///
    /// # Examples
    ///
    /// ```rust
    /// HotswapConfig::builder()
    ///     .with_file("config.yaml")
    ///     .with_validation(|config: &AppConfig| {
    ///         if config.server.port < 1024 {
    ///             return Err("Port must be >= 1024".into());
    ///         }
    ///         Ok(())
    ///     })
    ///     .build()?;
    /// ```
    pub fn with_validation<F, T>(self, validator: F) -> HotswapConfigValidationBuilder<T, F>
    where
        F: Fn(&T) -> Result<(), ValidationError> + Send + Sync + 'static,
        T: DeserializeOwned + Clone + Send + Sync + 'static,
    {
        // Returns specialized builder with type info
    }

    /// Enable configuration history for rollback support.
    ///
    /// # Arguments
    ///
    /// * `max_history` - Maximum number of historical versions to keep
    #[cfg(feature = "rollback")]
    pub fn with_history(mut self, max_history: usize) -> Self {
        self.history_size = max_history;
        self
    }

    /// Enable built-in metrics collection.
    #[cfg(feature = "metrics")]
    pub fn with_metrics(mut self, enabled: bool) -> Self {
        self.metrics_enabled = enabled;
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
    pub async fn build<T>(self) -> Result<HotswapConfig<T>, ConfigError>
    where
        T: DeserializeOwned + Clone + Send + Sync + 'static,
    {
        // Implementation
    }
}
```

### 3. Configuration Source Trait

```rust
use async_trait::async_trait;

/// Trait for configuration sources.
///
/// Implement this trait to create custom configuration sources (e.g., remote APIs,
/// databases, key-value stores).
#[async_trait]
pub trait ConfigSource: Send + Sync {
    /// Load configuration as a raw value map.
    ///
    /// The returned map will be merged with other sources according to precedence rules.
    async fn load(&self) -> Result<HashMap<String, serde_json::Value>, ConfigError>;

    /// Check if this source supports watching for changes.
    fn supports_watch(&self) -> bool {
        false
    }

    /// Start watching for changes (if supported).
    ///
    /// When changes are detected, the callback should be invoked.
    async fn watch<F>(&self, _callback: F) -> Result<(), ConfigError>
    where
        F: Fn() -> Result<(), ConfigError> + Send + Sync + 'static,
    {
        Err(ConfigError::WatchNotSupported)
    }

    /// Get a human-readable name for this source (for logging/debugging).
    fn name(&self) -> String;

    /// Get the priority of this source (higher = takes precedence).
    ///
    /// Default priorities:
    /// - Environment variables: 300
    /// - Environment-specific file: 200
    /// - Default file: 100
    /// - Remote sources: 50
    fn priority(&self) -> i32 {
        100
    }
}
```

### 4. Validation Trait

```rust
/// Trait for configuration validation.
///
/// Implement this trait on your configuration types to enable automatic validation.
pub trait Validate {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Validate the configuration.
    ///
    /// # Errors
    ///
    /// Should return an error describing what validation failed.
    fn validate(&self) -> Result<(), Self::Error>;
}

/// Validation error type.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Validation failed: {0}")]
    Custom(String),

    #[error("Field '{field}' has invalid value: {reason}")]
    InvalidField {
        field: String,
        reason: String,
    },

    #[error("Multiple validation errors: {0:?}")]
    Multiple(Vec<ValidationError>),
}
```

## Killer Features Design

### 1. Partial Updates

```rust
#[cfg(feature = "partial-updates")]
pub mod partial {
    use serde_json::Value;

    /// Apply a partial update to the configuration.
    ///
    /// This uses JSON Patch (RFC 6902) to apply surgical updates to specific
    /// configuration fields without requiring a full reload.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use serde_json::json;
    ///
    /// // Update only the rate limit
    /// config_handle.apply_patch(json!([
    ///     { "op": "replace", "path": "/rate_limit/max", "value": 1000 }
    /// ])).await?;
    /// ```
    impl<T: Clone + Serialize + DeserializeOwned> HotswapConfig<T> {
        pub async fn apply_patch(&self, patch: Value) -> Result<(), ConfigError> {
            let current = self.get();
            let current_json = serde_json::to_value(&*current)?;

            let patched = json_patch::patch(current_json, &patch)?;
            let new_config: T = serde_json::from_value(patched)?;

            self.update(new_config).await
        }

        pub async fn update_field<V: Serialize>(
            &self,
            path: &str,
            value: V,
        ) -> Result<(), ConfigError> {
            let patch = json!([
                { "op": "replace", "path": path, "value": value }
            ]);
            self.apply_patch(patch).await
        }
    }
}
```

### 2. Rollback Support

```rust
#[cfg(feature = "rollback")]
pub mod rollback {
    use chrono::{DateTime, Utc};

    /// A historical configuration version.
    #[derive(Debug, Clone)]
    pub struct ConfigVersion<T> {
        pub version: u64,
        pub config: Arc<T>,
        pub timestamp: DateTime<Utc>,
        pub source: String, // "file", "manual", "remote", etc.
    }

    /// Configuration history manager.
    pub struct ConfigHistory<T> {
        versions: Arc<RwLock<VecDeque<ConfigVersion<T>>>>,
        max_size: usize,
    }

    impl<T: Clone> ConfigHistory<T> {
        pub fn new(max_size: usize) -> Self {
            Self {
                versions: Arc::new(RwLock::new(VecDeque::with_capacity(max_size))),
                max_size,
            }
        }

        pub fn push(&self, version: ConfigVersion<T>) {
            let mut versions = self.versions.write();
            if versions.len() >= self.max_size {
                versions.pop_front();
            }
            versions.push_back(version);
        }

        pub fn get(&self, steps_back: usize) -> Option<ConfigVersion<T>> {
            let versions = self.versions.read();
            let idx = versions.len().checked_sub(steps_back + 1)?;
            versions.get(idx).cloned()
        }

        pub fn all(&self) -> Vec<ConfigVersion<T>> {
            self.versions.read().iter().cloned().collect()
        }
    }
}
```

### 3. Gradual Rollout

```rust
#[cfg(feature = "gradual-rollout")]
pub mod gradual {
    /// Gradual rollout strategy for A/B testing configuration changes.
    ///
    /// Allows deploying configuration changes to a percentage of requests
    /// or users before rolling out to everyone.
    pub struct GradualRollout<T> {
        stable: Arc<T>,
        canary: Arc<T>,
        canary_percentage: AtomicU8, // 0-100
    }

    impl<T: Clone> GradualRollout<T> {
        pub fn new(stable: T, canary: T, percentage: u8) -> Self {
            Self {
                stable: Arc::new(stable),
                canary: Arc::new(canary),
                canary_percentage: AtomicU8::new(percentage.min(100)),
            }
        }

        /// Get configuration based on rollout percentage.
        ///
        /// # Arguments
        ///
        /// * `key` - Optional key for consistent hashing (e.g., user_id)
        ///
        /// If no key is provided, uses random selection.
        pub fn get(&self, key: Option<&str>) -> Arc<T> {
            let percentage = self.canary_percentage.load(Ordering::Relaxed);

            if percentage == 0 {
                return self.stable.clone();
            }
            if percentage == 100 {
                return self.canary.clone();
            }

            let threshold = if let Some(k) = key {
                // Consistent hashing based on key
                self.hash_key(k) % 100
            } else {
                // Random selection
                fastrand::u8(0..100)
            };

            if threshold < percentage {
                self.canary.clone()
            } else {
                self.stable.clone()
            }
        }

        /// Gradually increase canary percentage.
        pub fn increase_canary(&self, delta: u8) {
            self.canary_percentage.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |current| Some((current + delta).min(100))
            ).ok();
        }

        /// Promote canary to stable (100% rollout).
        pub fn promote(&self) {
            self.canary_percentage.store(100, Ordering::Relaxed);
        }

        /// Rollback to stable (0% canary).
        pub fn rollback(&self) {
            self.canary_percentage.store(0, Ordering::Relaxed);
        }

        fn hash_key(&self, key: &str) -> u8 {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let mut hasher = DefaultHasher::new();
            key.hash(&mut hasher);
            (hasher.finish() % 100) as u8
        }
    }

    impl<T: Clone + Send + Sync + 'static> HotswapConfig<T> {
        /// Enable gradual rollout for the next configuration update.
        ///
        /// # Examples
        ///
        /// ```rust
        /// // Start with 10% traffic on new config
        /// config_handle.enable_gradual_rollout(10).await?;
        /// config_handle.reload().await?;
        ///
        /// // Monitor metrics, then increase
        /// config_handle.increase_rollout(50).await?;
        ///
        /// // Fully promote
        /// config_handle.promote_rollout().await?;
        /// ```
        pub async fn enable_gradual_rollout(&self, percentage: u8) -> Result<(), ConfigError> {
            // Implementation
        }
    }
}
```

### 4. Secret Management Integration

```rust
#[cfg(feature = "secrets-vault")]
pub mod secrets {
    use serde::Deserialize;

    /// Marker for fields that should be loaded from secret management.
    ///
    /// # Examples
    ///
    /// ```rust
    /// #[derive(Deserialize, Clone)]
    /// struct DatabaseConfig {
    ///     host: String,
    ///     port: u16,
    ///
    ///     #[serde(deserialize_with = "secret_from_vault")]
    ///     password: Secret<String>,
    /// }
    /// ```
    #[derive(Clone)]
    pub struct Secret<T> {
        value: Arc<T>,
    }

    impl<T> Secret<T> {
        pub fn reveal(&self) -> &T {
            &self.value
        }
    }

    /// Vault configuration source.
    pub struct VaultSource {
        client: vaultrs::client::VaultClient,
        mount: String,
        path: String,
    }

    impl VaultSource {
        pub fn new(addr: &str, token: &str, mount: &str, path: &str) -> Result<Self, ConfigError> {
            // Implementation
        }
    }

    #[async_trait]
    impl ConfigSource for VaultSource {
        async fn load(&self) -> Result<HashMap<String, Value>, ConfigError> {
            // Load secrets from Vault
        }

        fn name(&self) -> String {
            format!("vault:{}/{}", self.mount, self.path)
        }

        fn priority(&self) -> i32 {
            350 // Higher than env vars
        }
    }
}
```

## Built-in Metrics

```rust
#[cfg(feature = "metrics")]
pub mod metrics {
    /// Built-in metrics tracked by hotswap-config.
    pub struct ConfigMetrics {
        /// Total number of reload attempts
        reload_attempts: Counter,

        /// Successful reloads
        reload_success: Counter,

        /// Failed reloads
        reload_failures: Counter,

        /// Reload duration histogram
        reload_duration: Histogram,

        /// Configuration age (time since last update)
        config_age_seconds: Gauge,

        /// Number of active subscribers
        active_subscribers: Gauge,

        /// Validation failures
        validation_failures: Counter,

        /// Current configuration version
        #[cfg(feature = "rollback")]
        config_version: Gauge,
    }

    impl ConfigMetrics {
        pub fn new(meter: &opentelemetry::metrics::Meter) -> Self {
            // Initialize all metrics
        }

        pub fn record_reload(&self, duration: Duration, success: bool) {
            // Record reload metrics
        }
    }
}
```

## Usage Examples

### Basic Usage

```rust
use hotswap_config::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    server: ServerConfig,
    database: DatabaseConfig,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration with standard precedence
    let config = HotswapConfig::builder()
        .with_file("config/default.yaml")
        .with_file("config/production.yaml")
        .with_env_overrides("APP", "__")
        .build::<AppConfig>()
        .await?;

    // Zero-cost reads
    let cfg = config.get();
    println!("Server port: {}", cfg.server.port);

    // Config handle is cloneable and cheap to pass around
    let config_clone = config.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let cfg = config_clone.get();
            println!("Still running on port {}", cfg.server.port);
        }
    });

    Ok(())
}
```

### Hot Reload with Validation

```rust
use hotswap_config::prelude::*;

#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    rate_limit: RateLimitConfig,
}

impl Validate for AppConfig {
    type Error = ValidationError;

    fn validate(&self) -> Result<(), Self::Error> {
        if self.rate_limit.max < 10 {
            return Err(ValidationError::invalid_field(
                "rate_limit.max",
                "must be at least 10"
            ));
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = HotswapConfig::builder()
        .with_file("config/default.yaml")
        .with_file_watch(true) // Auto-reload on file changes
        .with_validation(|cfg: &AppConfig| cfg.validate())
        .build::<AppConfig>()
        .await?;

    // Subscribe to changes
    config.subscribe(|new_cfg| {
        info!("Config updated! New rate limit: {}", new_cfg.rate_limit.max);
    });

    // Changes to config files are automatically picked up
    // Invalid configs are rejected and old config is retained

    Ok(())
}
```

### Gradual Rollout

```rust
use hotswap_config::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = HotswapConfig::builder()
        .with_file("config/stable.yaml")
        .build::<AppConfig>()
        .await?;

    // Enable gradual rollout with new config
    config.enable_gradual_rollout(10).await?; // 10% traffic
    config.load_canary("config/canary.yaml").await?;

    // Monitor metrics for 1 hour
    tokio::time::sleep(Duration::from_secs(3600)).await;

    // Gradually increase if metrics look good
    config.increase_rollout(25).await?; // 35% now
    tokio::time::sleep(Duration::from_secs(3600)).await;

    config.increase_rollout(65).await?; // 100% now

    // Promote canary to stable
    config.promote_rollout().await?;

    Ok(())
}
```

### Partial Updates

```rust
use hotswap_config::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = HotswapConfig::builder()
        .with_file("config/default.yaml")
        .build::<AppConfig>()
        .await?;

    // Update just the rate limit without full reload
    config.update_field("/rate_limit/max", 5000).await?;

    // Or use JSON Patch for complex updates
    config.apply_patch(json!([
        { "op": "replace", "path": "/rate_limit/max", "value": 5000 },
        { "op": "replace", "path": "/rate_limit/burst", "value": 10000 },
    ])).await?;

    Ok(())
}
```

### Rollback Support

```rust
use hotswap_config::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = HotswapConfig::builder()
        .with_file("config/default.yaml")
        .with_history(20) // Keep last 20 versions
        .build::<AppConfig>()
        .await?;

    // Make some changes
    config.reload().await?; // Version 2
    config.reload().await?; // Version 3
    config.reload().await?; // Version 4

    // Something went wrong, rollback!
    config.rollback(2).await?; // Back to version 2

    // View history
    for version in config.history() {
        println!("Version {}: loaded at {}", version.version, version.timestamp);
    }

    Ok(())
}
```

### Remote Configuration

```rust
use hotswap_config::prelude::*;
use hotswap_config::sources::HttpSource;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let remote_source = HttpSource::builder()
        .with_url("https://config-server.example.com/api/config")
        .with_auth_token("secret-token")
        .with_poll_interval(Duration::from_secs(30))
        .build()?;

    let config = HotswapConfig::builder()
        .with_file("config/default.yaml")  // Fallback
        .with_source(remote_source)        // Higher priority
        .build::<AppConfig>()
        .await?;

    Ok(())
}
```

## Dependencies

```toml
[dependencies]
# Core
serde = { version = "1.0", features = ["derive"] }
arc-swap = "1.7"
tokio = { version = "1.0", features = ["sync", "time"], optional = true }

# Config loading
config = "0.14"

# Optional features
notify = { version = "6.0", optional = true }
serde_yaml = { version = "0.9", optional = true }
toml = { version = "0.8", optional = true }
serde_json = { version = "1.0", optional = true }
json-patch = { version = "2.0", optional = true }

# Remote sources
reqwest = { version = "0.12", features = ["json"], optional = true }
async-trait = { version = "0.1", optional = true }

# Secret management
vaultrs = { version = "0.7", optional = true }
aws-sdk-secretsmanager = { version = "1.0", optional = true }
google-secretmanager = { version = "0.1", optional = true }

# Observability
opentelemetry = { version = "0.30", optional = true }
tracing = { version = "0.1", optional = true }

# Utilities
thiserror = "1.0"
fastrand = { version = "2.0", optional = true }
chrono = { version = "0.4", optional = true }

[dev-dependencies]
tokio = { version = "1.0", features = ["full"] }
tokio-test = "0.4"
tempfile = "3.0"
```

## Testing Strategy

1. **Unit Tests**: Test individual components in isolation
2. **Integration Tests**: Test full reload cycles with multiple sources
3. **Concurrent Tests**: Verify lock-free behavior under high concurrency
4. **Property Tests**: Use proptest for validation logic
5. **Benchmark Tests**: Measure read/write performance with criterion

## Documentation Plan

1. **API Documentation**: Comprehensive rustdoc for all public APIs
2. **Examples Directory**: Complete working examples for each feature
3. **User Guide**: Step-by-step guide in README.md
4. **Migration Guide**: How to migrate from `config` crate
5. **Performance Guide**: Best practices for optimal performance
6. **Security Guide**: Best practices for secret management

## Roadmap

### Phase 1: Core (v0.1.0)
- [ ] Core config loading with precedence
- [ ] Arc-swap based hot reload
- [ ] File watching
- [ ] Validation trait
- [ ] Basic metrics
- [ ] Documentation and examples

### Phase 2: Advanced (v0.2.0)
- [ ] Partial updates
- [ ] Rollback support
- [ ] Gradual rollout
- [ ] Remote sources (HTTP)
- [ ] Enhanced metrics

### Phase 3: Enterprise (v0.3.0)
- [ ] Secret management integration (Vault, AWS, GCP)
- [ ] Distributed configuration (etcd, Consul)
- [ ] WebAssembly support
- [ ] Configuration UI/dashboard

## Success Metrics

- **Performance**: < 10ns read latency (measured with criterion)
- **Reliability**: Zero dropped requests during reload (load test)
- **Adoption**: 100+ GitHub stars within 6 months
- **Quality**: > 90% test coverage
- **Community**: 5+ external contributors within 1 year
