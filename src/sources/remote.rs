//! Remote HTTP/HTTPS configuration source.

use super::ConfigSource;
use crate::error::{ConfigError, Result};
use reqwest::{Client, header::HeaderValue};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Authentication method for HTTP requests.
#[derive(Clone)]
pub enum HttpAuth {
    /// No authentication
    None,
    /// Bearer token authentication
    Bearer(String),
    /// Basic authentication (username, password)
    Basic(String, String),
}

/// HTTP-based configuration source.
///
/// Fetches configuration from a remote HTTP/HTTPS endpoint. Supports authentication,
/// configurable timeouts, and caches the last-known-good configuration on errors.
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::sources::HttpSource;
/// use std::time::Duration;
///
/// # async fn example() -> hotswap_config::error::Result<()> {
/// let source = HttpSource::builder()
///     .with_url("https://config.example.com/api/config")
///     .with_auth_token("secret-token")
///     .with_timeout(Duration::from_secs(10))
///     .with_priority(250)
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct HttpSource {
    url: String,
    client: Client,
    auth: HttpAuth,
    priority: i32,
    last_known_good: Arc<RwLock<Option<HashMap<String, config::Value>>>>,
}

impl HttpSource {
    /// Create a new builder for constructing an HTTP source.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::HttpSource;
    ///
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// let source = HttpSource::builder()
    ///     .with_url("https://config.example.com/api/config")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder() -> HttpSourceBuilder {
        HttpSourceBuilder::new()
    }

    /// Fetch configuration from the remote endpoint.
    async fn fetch(&self) -> Result<HashMap<String, config::Value>> {
        let mut request = self.client.get(&self.url);

        // Add authentication headers
        request = match &self.auth {
            HttpAuth::None => request,
            HttpAuth::Bearer(token) => {
                let header_value = HeaderValue::from_str(&format!("Bearer {}", token))
                    .map_err(|e| ConfigError::LoadError(format!("Invalid bearer token: {}", e)))?;
                request.header("Authorization", header_value)
            }
            HttpAuth::Basic(username, password) => request.basic_auth(username, Some(password)),
        };

        // Send request
        let response = request
            .send()
            .await
            .map_err(|e| ConfigError::LoadError(format!("HTTP request failed: {}", e)))?;

        // Check status code
        let status = response.status();
        if !status.is_success() {
            return Err(ConfigError::LoadError(format!(
                "HTTP request failed with status {}: {}",
                status,
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        // Parse JSON response
        let json: JsonValue = response.json().await.map_err(|e| {
            ConfigError::DeserializationError(format!("Failed to parse JSON: {}", e))
        })?;

        // Convert JSON to config::Value HashMap
        let map = json_to_config_map(json)?;

        // Cache as last known good
        *self.last_known_good.write().unwrap() = Some(map.clone());

        Ok(map)
    }
}

impl ConfigSource for HttpSource {
    fn load(&self) -> Result<HashMap<String, config::Value>> {
        // ConfigSource::load() is synchronous but fetch() is async.
        // Use block_in_place when inside an existing runtime to avoid panics.
        // NOTE: block_in_place requires the multi-thread runtime (rt-multi-thread).
        // Using HttpSource with current_thread runtime is not supported.
        #[cfg(feature = "tokio-runtime")]
        {
            let handle = tokio::runtime::Handle::try_current();
            match handle {
                Ok(handle) => {
                    // Inside a runtime — use block_in_place to safely bridge sync/async
                    tokio::task::block_in_place(|| {
                        handle.block_on(async { self.fetch().await })
                    })
                }
                Err(_) => {
                    // No runtime — create a temporary one
                    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
                        ConfigError::LoadError(format!("Failed to create runtime: {}", e))
                    })?;
                    runtime.block_on(async { self.fetch().await })
                }
            }
        }

        #[cfg(not(feature = "tokio-runtime"))]
        {
            Err(ConfigError::LoadError(
                "HttpSource requires the 'tokio-runtime' feature".to_string(),
            ))
        }
    }

    fn name(&self) -> String {
        format!("http:{}", self.url)
    }

    fn priority(&self) -> i32 {
        self.priority
    }
}

/// Builder for constructing an `HttpSource`.
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::sources::HttpSource;
/// use std::time::Duration;
///
/// # async fn example() -> hotswap_config::error::Result<()> {
/// let source = HttpSource::builder()
///     .with_url("https://config.example.com/api/config")
///     .with_auth_token("secret-token")
///     .with_timeout(Duration::from_secs(10))
///     .with_priority(250)
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct HttpSourceBuilder {
    url: Option<String>,
    auth: HttpAuth,
    timeout: Duration,
    priority: i32,
}

impl HttpSourceBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            url: None,
            auth: HttpAuth::None,
            timeout: Duration::from_secs(10),
            priority: 250, // Higher than files (100-200), lower than env vars (300)
        }
    }

    /// Set the URL to fetch configuration from.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::HttpSource;
    ///
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// HttpSource::builder()
    ///     .with_url("https://config.example.com/api/config");
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Set Bearer token authentication.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::HttpSource;
    ///
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// HttpSource::builder()
    ///     .with_url("https://config.example.com/api/config")
    ///     .with_auth_token("secret-token");
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth = HttpAuth::Bearer(token.into());
        self
    }

    /// Set Basic authentication.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::HttpSource;
    ///
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// HttpSource::builder()
    ///     .with_url("https://config.example.com/api/config")
    ///     .with_basic_auth("username", "password");
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_basic_auth(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.auth = HttpAuth::Basic(username.into(), password.into());
        self
    }

    /// Set the request timeout.
    ///
    /// Default is 10 seconds.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::HttpSource;
    /// use std::time::Duration;
    ///
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// HttpSource::builder()
    ///     .with_url("https://config.example.com/api/config")
    ///     .with_timeout(Duration::from_secs(5));
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the priority for this source.
    ///
    /// Default is 250 (higher than files, lower than environment variables).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::HttpSource;
    ///
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// HttpSource::builder()
    ///     .with_url("https://config.example.com/api/config")
    ///     .with_priority(150);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Build the HTTP source.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No URL is provided
    /// - The HTTP client cannot be constructed
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::HttpSource;
    ///
    /// # async fn example() -> hotswap_config::error::Result<()> {
    /// let source = HttpSource::builder()
    ///     .with_url("https://config.example.com/api/config")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build(self) -> Result<HttpSource> {
        let url = self
            .url
            .ok_or_else(|| ConfigError::LoadError("URL is required for HttpSource".to_string()))?;

        let client = Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| ConfigError::LoadError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(HttpSource {
            url,
            client,
            auth: self.auth,
            priority: self.priority,
            last_known_good: Arc::new(RwLock::new(None)),
        })
    }
}

impl Default for HttpSourceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a JSON value to a config::Value HashMap.
fn json_to_config_map(json: JsonValue) -> Result<HashMap<String, config::Value>> {
    match json {
        JsonValue::Object(map) => {
            let mut result = HashMap::new();
            for (key, value) in map {
                result.insert(key, json_value_to_config_value(value)?);
            }
            Ok(result)
        }
        _ => Err(ConfigError::DeserializationError(
            "Expected JSON object at root level".to_string(),
        )),
    }
}

/// Convert a serde_json::Value to a config::Value.
fn json_value_to_config_value(value: JsonValue) -> Result<config::Value> {
    match value {
        JsonValue::Null => Ok(config::Value::new(None, config::ValueKind::Nil)),
        JsonValue::Bool(b) => Ok(config::Value::new(None, config::ValueKind::Boolean(b))),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(config::Value::new(None, config::ValueKind::I64(i)))
            } else if let Some(f) = n.as_f64() {
                Ok(config::Value::new(None, config::ValueKind::Float(f)))
            } else {
                Err(ConfigError::DeserializationError(format!(
                    "Unsupported number type: {}",
                    n
                )))
            }
        }
        JsonValue::String(s) => Ok(config::Value::new(None, config::ValueKind::String(s))),
        JsonValue::Array(arr) => {
            let values: Result<Vec<config::Value>> =
                arr.into_iter().map(json_value_to_config_value).collect();
            Ok(config::Value::new(None, config::ValueKind::Array(values?)))
        }
        JsonValue::Object(map) => {
            let mut result = HashMap::new();
            for (key, val) in map {
                result.insert(key, json_value_to_config_value(val)?);
            }
            Ok(config::Value::new(None, config::ValueKind::Table(result)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder() {
        let builder = HttpSource::builder()
            .with_url("https://example.com/config")
            .with_auth_token("token123")
            .with_timeout(Duration::from_secs(5))
            .with_priority(200);

        let source = builder.build();
        assert!(source.is_ok());

        let source = source.unwrap();
        assert_eq!(source.url, "https://example.com/config");
        assert_eq!(source.priority(), 200);
    }

    #[test]
    fn test_builder_no_url() {
        let builder = HttpSource::builder();
        let source = builder.build();
        assert!(source.is_err());
    }

    #[test]
    fn test_builder_with_basic_auth() {
        let source = HttpSource::builder()
            .with_url("https://example.com/config")
            .with_basic_auth("user", "pass")
            .build();

        assert!(source.is_ok());
    }

    #[test]
    fn test_json_to_config_map() {
        use serde_json::json;

        let json = json!({
            "server": {
                "port": 8080,
                "host": "localhost"
            },
            "debug": true
        });

        let map = json_to_config_map(json);
        assert!(map.is_ok());

        let map = map.unwrap();
        assert!(map.contains_key("server"));
        assert!(map.contains_key("debug"));
    }

    #[test]
    fn test_json_to_config_map_invalid() {
        use serde_json::json;

        let json = json!([1, 2, 3]); // Array at root, not object

        let map = json_to_config_map(json);
        assert!(map.is_err());
    }
}
