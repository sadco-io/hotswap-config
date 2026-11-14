//! Partial configuration updates using JSON Patch.
//!
//! Allows surgical updates to configuration without reloading from files.
//!
//! # Serialization Caveat
//!
//! [`PartialUpdate`] works by serializing the current `T` to a `serde_json::Value`,
//! applying RFC 6902 JSON Patch operations, and deserializing the result back into `T`.
//! Avoid annotating fields on `T` with `#[serde(skip_serializing)]` when using
//! `PartialUpdate`: skipped fields are omitted from the intermediate JSON object,
//! causing deserialization to fail (or reset to `#[serde(default)]`). To prevent
//! secrets from appearing in logs, implement a custom [`std::fmt::Debug`] that
//! redacts sensitive fields instead of skipping serde serialization.

use crate::core::HotswapConfig;
use crate::error::{ConfigError, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::sync::Arc;

/// Extension trait for partial configuration updates.
///
/// Provides methods for applying JSON Patch operations and updating individual fields.
/// Concurrent patch operations on the same [`HotswapConfig`] are serialized so field
/// updates are never lost to read-modify-write races.
pub trait PartialUpdate<T> {
    /// Apply a JSON Patch to the configuration.
    ///
    /// The patch is applied to the serialized configuration under the handle's write lock,
    /// then deserialized and validated before atomically swapping.
    ///
    /// # Arguments
    ///
    /// * `patch` - JSON Patch document (array of RFC 6902 operations)
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`ConfigError::PatchError`] if the patch is malformed or cannot be applied
    /// - [`ConfigError::DeserializationError`] if the patched JSON cannot deserialize into `T`
    /// - [`ConfigError::ValidationError`] if validation fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use hotswap_config::features::PartialUpdate;
    /// use serde::Deserialize;
    /// use serde_json::json;
    ///
    /// #[derive(Debug, Deserialize, Clone, serde::Serialize)]
    /// struct AppConfig {
    ///     port: u16,
    ///     host: String,
    /// }
    ///
    /// # async fn example(config: HotswapConfig<AppConfig>) -> Result<()> {
    /// // Change just the port
    /// let patch = json!([
    ///     { "op": "replace", "path": "/port", "value": 9090 }
    /// ]);
    ///
    /// config.apply_patch(patch).await?;
    /// # Ok(())
    /// # }
    /// ```
    fn apply_patch(&self, patch: Value) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Update a single field in the configuration.
    ///
    /// This is a convenience method that creates a JSON Patch `replace` operation.
    ///
    /// # Arguments
    ///
    /// * `path` - JSON Pointer path to the field (e.g., `"/port"`, `"/database/host"`)
    /// * `value` - New value for the field (must be serializable)
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::PatchError`] if the path is invalid, or
    /// [`ConfigError::ValidationError`] if validation fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::prelude::*;
    /// use hotswap_config::features::PartialUpdate;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, Clone, serde::Serialize)]
    /// struct AppConfig {
    ///     port: u16,
    /// }
    ///
    /// # async fn example(config: HotswapConfig<AppConfig>) -> Result<()> {
    /// config.update_field("/port", 9090).await?;
    /// # Ok(())
    /// # }
    /// ```
    fn update_field<V: Serialize + Send>(
        &self,
        path: &str,
        value: V,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

impl<T> PartialUpdate<T> for HotswapConfig<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
{
    async fn apply_patch(&self, patch: Value) -> Result<()> {
        // Parse patch upfront before acquiring write_lock
        let parsed_patch: json_patch::Patch = serde_json::from_value(patch)
            .map_err(|e| ConfigError::PatchError(format!("Invalid JSON Patch: {}", e)))?;

        {
            let _guard = self
                .inner
                .write_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());

            // Read latest committed config while holding write_lock (prevents lost updates)
            let current = self.inner.current.load_full();

            let mut current_json = serde_json::to_value(&*current).map_err(|e| {
                ConfigError::PatchError(format!("Failed to serialize config: {}", e))
            })?;

            json_patch::patch(&mut current_json, &parsed_patch)
                .map_err(|e| ConfigError::PatchError(format!("Failed to apply patch: {}", e)))?;

            let new_config: T = serde_json::from_value(current_json).map_err(|e| {
                ConfigError::DeserializationError(format!(
                    "Failed to deserialize patched config: {}",
                    e
                ))
            })?;

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
                history.record_sync(arc_config, Some("Partial update".to_string()));
            }

            #[cfg(feature = "metrics")]
            if let Some(metrics) = &self.inner.metrics {
                metrics.record_update();
            }
        }

        self.inner.notify_subscribers().await;
        Ok(())
    }

    async fn update_field<V: Serialize + Send>(&self, path: &str, value: V) -> Result<()> {
        let value_json = serde_json::to_value(value)
            .map_err(|e| ConfigError::PatchError(format!("Failed to serialize value: {}", e)))?;

        let patch = serde_json::json!([
            { "op": "replace", "path": path, "value": value_json }
        ]);

        self.apply_patch(patch).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestConfig {
        port: u16,
        host: String,
        database: DatabaseConfig,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct DatabaseConfig {
        url: String,
        pool_size: u32,
    }

    #[tokio::test]
    async fn test_apply_patch_replace() {
        let initial = TestConfig {
            port: 8080,
            host: "localhost".to_string(),
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                pool_size: 10,
            },
        };

        let config = HotswapConfig::new(initial);

        let patch = json!([
            { "op": "replace", "path": "/port", "value": 9090 }
        ]);

        config.apply_patch(patch).await.unwrap();

        let updated = config.get();
        assert_eq!(updated.port, 9090);
        assert_eq!(updated.host, "localhost");
    }

    #[tokio::test]
    async fn test_apply_patch_nested() {
        let initial = TestConfig {
            port: 8080,
            host: "localhost".to_string(),
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                pool_size: 10,
            },
        };

        let config = HotswapConfig::new(initial);

        let patch = json!([
            { "op": "replace", "path": "/database/pool_size", "value": 20 }
        ]);

        config.apply_patch(patch).await.unwrap();

        let updated = config.get();
        assert_eq!(updated.database.pool_size, 20);
    }

    #[tokio::test]
    async fn test_apply_patch_multiple_ops() {
        let initial = TestConfig {
            port: 8080,
            host: "localhost".to_string(),
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                pool_size: 10,
            },
        };

        let config = HotswapConfig::new(initial);

        let patch = json!([
            { "op": "replace", "path": "/port", "value": 9090 },
            { "op": "replace", "path": "/host", "value": "0.0.0.0" }
        ]);

        config.apply_patch(patch).await.unwrap();

        let updated = config.get();
        assert_eq!(updated.port, 9090);
        assert_eq!(updated.host, "0.0.0.0");
    }

    #[tokio::test]
    async fn test_update_field() {
        let initial = TestConfig {
            port: 8080,
            host: "localhost".to_string(),
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                pool_size: 10,
            },
        };

        let config = HotswapConfig::new(initial);

        config.update_field("/port", 9090).await.unwrap();

        let updated = config.get();
        assert_eq!(updated.port, 9090);
    }

    #[tokio::test]
    async fn test_update_nested_field() {
        let initial = TestConfig {
            port: 8080,
            host: "localhost".to_string(),
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                pool_size: 10,
            },
        };

        let config = HotswapConfig::new(initial);

        config
            .update_field("/database/url", "postgres://remote/db")
            .await
            .unwrap();

        let updated = config.get();
        assert_eq!(updated.database.url, "postgres://remote/db");
    }

    #[tokio::test]
    async fn test_invalid_patch_returns_patch_error() {
        let initial = TestConfig {
            port: 8080,
            host: "localhost".to_string(),
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                pool_size: 10,
            },
        };

        let config = HotswapConfig::new(initial);

        let patch = json!([
            { "op": "invalid", "path": "/port" }
        ]);

        let result = config.apply_patch(patch).await;
        assert!(matches!(result, Err(ConfigError::PatchError(_))));
    }

    #[tokio::test]
    async fn test_invalid_path_returns_patch_error() {
        let initial = TestConfig {
            port: 8080,
            host: "localhost".to_string(),
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                pool_size: 10,
            },
        };

        let config = HotswapConfig::new(initial);

        let result = config.update_field("/nonexistent", 123).await;
        assert!(matches!(result, Err(ConfigError::PatchError(_))));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_update_field_serialization_no_lost_updates() {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
        struct MapConfig {
            slots: HashMap<String, usize>,
        }

        let mut slots = HashMap::new();
        for i in 0..20 {
            slots.insert(format!("key_{}", i), 0);
        }
        let config = HotswapConfig::new(MapConfig { slots });

        let mut handles = Vec::new();
        for i in 0..20usize {
            let cfg = config.clone();
            handles.push(tokio::spawn(async move {
                let path = format!("/slots/key_{}", i);
                cfg.update_field(&path, i + 100).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let final_cfg = config.get();
        for i in 0..20usize {
            assert_eq!(
                final_cfg.slots.get(&format!("key_{}", i)),
                Some(&(i + 100)),
                "Concurrent update_field lost update for key_{}",
                i
            );
        }
    }
}
