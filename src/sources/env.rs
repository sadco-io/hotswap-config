//! Environment variable configuration source.

use super::ConfigSource;
use crate::error::Result;
use config::Environment;
use std::collections::HashMap;

/// Environment variable configuration source.
///
/// Loads configuration from environment variables with a specified prefix,
/// prefix separator (default `"_"`), and nested-key separator.
///
/// # Examples
///
/// ```rust
/// use hotswap_config::sources::EnvSource;
///
/// // APP_SERVER__PORT=8080 -> server.port = 8080
/// // APP_SERVER_PORT=8080  -> server_port = 8080
/// let source = EnvSource::new("APP", "__");
/// ```
#[derive(Debug, Clone)]
pub struct EnvSource {
    prefix: String,
    prefix_separator: String,
    separator: String,
    priority: i32,
}

impl EnvSource {
    /// Create a new environment variable source.
    ///
    /// Uses `"_"` as the default separator between `prefix` and the variable name,
    /// while `separator` splits nested struct paths (e.g., `"__"` turns
    /// `APP_DB__HOST` into `db.host` while preserving snake_case field names like
    /// `APP_MAX_CONNECTIONS` -> `max_connections`).
    ///
    /// # Arguments
    ///
    /// * `prefix` - Prefix for environment variables (e.g., `"APP"`)
    /// * `separator` - Separator for nested keys (e.g., `"__"` for `APP_DB__HOST`)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hotswap_config::sources::EnvSource;
    ///
    /// // Matches: APP_SERVER__PORT, APP_DB__HOST, APP_SERVER_PORT, etc.
    /// let source = EnvSource::new("APP", "__");
    /// ```
    pub fn new(prefix: impl Into<String>, separator: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            prefix_separator: "_".to_string(),
            separator: separator.into(),
            priority: 300, // Env vars have highest priority by default
        }
    }

    /// Set a custom separator between `prefix` and the rest of the variable name.
    ///
    /// Defaults to `"_"`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hotswap_config::sources::EnvSource;
    ///
    /// // Matches APP__SERVER__PORT with "__" after prefix as well
    /// let source = EnvSource::new("APP", "__").with_prefix_separator("__");
    /// ```
    pub fn with_prefix_separator(mut self, prefix_separator: impl Into<String>) -> Self {
        self.prefix_separator = prefix_separator.into();
        self
    }

    /// Set the priority for this source.
    ///
    /// Higher priority sources override lower priority ones.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

impl ConfigSource for EnvSource {
    fn load(&self) -> Result<HashMap<String, config::Value>> {
        let trimmed_prefix = if self.prefix_separator.is_empty() {
            self.prefix.as_str()
        } else {
            self.prefix.trim_end_matches(&self.prefix_separator)
        };

        let env_source = if trimmed_prefix.is_empty() {
            Environment::default()
                .separator(&self.separator)
                .try_parsing(true)
        } else {
            Environment::with_prefix(trimmed_prefix)
                .prefix_separator(&self.prefix_separator)
                .separator(&self.separator)
                .try_parsing(true)
        };

        // Build a config with just this environment source
        let config_builder = config::Config::builder()
            .add_source(env_source)
            .build()
            .map_err(|e| {
                crate::error::ConfigError::LoadError(format!(
                    "Failed to load environment variables: {}",
                    e
                ))
            })?;

        // Extract as HashMap
        let map = config_builder
            .try_deserialize::<HashMap<String, config::Value>>()
            .map_err(|e| {
                crate::error::ConfigError::DeserializationError(format!(
                    "Failed to parse environment variables: {}",
                    e
                ))
            })?;

        Ok(map)
    }

    fn name(&self) -> String {
        format!("env:{}*", self.prefix)
    }

    fn priority(&self) -> i32 {
        self.priority
    }
}

#[cfg(test)]
#[allow(unsafe_code)] // For env var manipulation in tests
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_env_source_creation() {
        let source = EnvSource::new("APP", "__");
        assert_eq!(source.prefix, "APP");
        assert_eq!(source.prefix_separator, "_");
        assert_eq!(source.separator, "__");
        assert_eq!(source.priority(), 300);
        assert!(format!("{:?}", source).contains("EnvSource"));
    }

    #[test]
    fn test_with_priority_and_prefix_separator() {
        let source = EnvSource::new("APP", "__")
            .with_prefix_separator("__")
            .with_priority(400);
        assert_eq!(source.priority(), 400);
        assert_eq!(source.prefix_separator, "__");
    }

    #[test]
    fn test_name() {
        let source = EnvSource::new("APP", "__");
        assert_eq!(source.name(), "env:APP*");
    }

    #[test]
    fn test_load_empty() {
        for (key, _) in env::vars() {
            if key.starts_with("TEST_HOTSWAP_EMPTY_") {
                unsafe {
                    env::remove_var(&key);
                }
            }
        }

        let source = EnvSource::new("TEST_HOTSWAP_EMPTY", "__");
        let map = source.load().unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn test_load_nested_and_snake_case_keys() {
        unsafe {
            env::set_var("TEST_HOTSWAP_ENV_UNIT_SERVER__PORT", "9000");
            env::set_var("TEST_HOTSWAP_ENV_UNIT_SERVER_PORT", "8080");
        }

        let source = EnvSource::new("TEST_HOTSWAP_ENV_UNIT", "__");
        let map = source.load();

        unsafe {
            env::remove_var("TEST_HOTSWAP_ENV_UNIT_SERVER__PORT");
            env::remove_var("TEST_HOTSWAP_ENV_UNIT_SERVER_PORT");
        }

        let map = map.unwrap();
        assert_eq!(
            map.get("server_port")
                .and_then(|v| v.clone().into_int().ok()),
            Some(8080)
        );
        let server_table = map
            .get("server")
            .and_then(|v| v.clone().into_table().ok())
            .expect("nested server table should exist");
        assert_eq!(
            server_table
                .get("port")
                .and_then(|v| v.clone().into_int().ok()),
            Some(9000)
        );
    }
}
