//! Remote HTTP/HTTPS configuration source.

use super::ConfigSource;
use crate::error::{ConfigError, Result};
use reqwest::{Client, header::HeaderValue};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Redact any userinfo password (`scheme://user:password@host...`) in a URL string.
fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let Some(at_pos) = authority.rfind('@') else {
        return url.to_string();
    };
    let userinfo = &authority[..at_pos];
    let Some(colon_pos) = userinfo.find(':') else {
        return url.to_string();
    };
    let user = &userinfo[..colon_pos];
    let host_and_path = &rest[at_pos..];
    format!(
        "{}://{}:<redacted>{}",
        &url[..scheme_end],
        user,
        host_and_path
    )
}

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

impl fmt::Debug for HttpAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Bearer(_) => f.debug_tuple("Bearer").field(&"<redacted>").finish(),
            Self::Basic(username, _) => f
                .debug_tuple("Basic")
                .field(username)
                .field(&"<redacted>")
                .finish(),
        }
    }
}

/// HTTP-based configuration source.
///
/// Fetches configuration from a remote HTTP/HTTPS endpoint. Supports authentication,
/// configurable timeouts, and caches the last-known-good configuration on transport
/// or HTTP status errors when fallback is enabled (the default).
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
    fallback_to_cached: bool,
    last_known_good: Arc<RwLock<Option<HashMap<String, config::Value>>>>,
}

impl fmt::Debug for HttpSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let has_cached = self
            .last_known_good
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        f.debug_struct("HttpSource")
            .field("url", &redact_url(&self.url))
            .field("auth", &self.auth)
            .field("priority", &self.priority)
            .field("fallback_to_cached", &self.fallback_to_cached)
            .field("has_cached_config", &has_cached)
            .finish()
    }
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

    fn fallback_or_err(&self, err: ConfigError) -> Result<HashMap<String, config::Value>> {
        if self.fallback_to_cached {
            let guard = self
                .last_known_good
                .read()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = guard.as_ref() {
                return Ok(cached.clone());
            }
        }
        Err(err)
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

        // Send request (on transport failure, fall back to cached last-known-good if enabled)
        let response = match request.send().await {
            Ok(resp) => resp,
            Err(e) => {
                return self.fallback_or_err(ConfigError::LoadError(format!(
                    "HTTP request failed: {}",
                    e
                )));
            }
        };

        // Check status code (on 4xx/5xx, fall back to cached last-known-good if enabled)
        let status = response.status();
        if !status.is_success() {
            return self.fallback_or_err(ConfigError::LoadError(format!(
                "HTTP request failed with status {}: {}",
                status,
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        // Read response bytes (network drop mid-body falls back to cached config)
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(e) => {
                return self.fallback_or_err(ConfigError::LoadError(format!(
                    "Failed to read HTTP response body: {}",
                    e
                )));
            }
        };

        // Parse JSON response (parse errors deliberately fail to avoid silent corruption)
        let json: JsonValue = serde_json::from_slice(&bytes).map_err(|e| {
            ConfigError::DeserializationError(format!("Failed to parse JSON: {}", e))
        })?;

        // Convert JSON to config::Value HashMap
        let map = json_to_config_map(json)?;

        // Cache as last known good
        *self
            .last_known_good
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(map.clone());

        Ok(map)
    }
}

impl ConfigSource for HttpSource {
    fn load(&self) -> Result<HashMap<String, config::Value>> {
        // ConfigSource::load() is synchronous while reqwest is async.
        // Bridge safely from CurrentThread runtimes, MultiThread runtimes, and plain OS threads
        // without ever calling `block_in_place` (which panics on CurrentThread runtimes).
        #[cfg(feature = "tokio-runtime")]
        {
            match tokio::runtime::Handle::try_current() {
                Ok(handle)
                    if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread =>
                {
                    std::thread::scope(|s| {
                        s.spawn(|| handle.block_on(self.fetch()))
                            .join()
                            .map_err(|_| {
                                ConfigError::LoadError(
                                    "HTTP fetch worker thread panicked".to_string(),
                                )
                            })?
                    })
                }
                _ => std::thread::scope(|s| {
                    s.spawn(|| {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| {
                                ConfigError::LoadError(format!("Failed to create runtime: {}", e))
                            })?;
                        runtime.block_on(self.fetch())
                    })
                    .join()
                    .map_err(|_| {
                        ConfigError::LoadError("HTTP fetch worker thread panicked".to_string())
                    })?
                }),
            }
        }

        #[cfg(not(feature = "tokio-runtime"))]
        {
            Err(ConfigError::FeatureNotEnabled("tokio-runtime"))
        }
    }

    fn name(&self) -> String {
        format!("http:{}", redact_url(&self.url))
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
///     .with_fallback_to_cached(true)
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct HttpSourceBuilder {
    url: Option<String>,
    auth: HttpAuth,
    timeout: Duration,
    priority: i32,
    fallback_to_cached: bool,
}

impl fmt::Debug for HttpSourceBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted_url = self.url.as_deref().map(redact_url);
        f.debug_struct("HttpSourceBuilder")
            .field("url", &redacted_url)
            .field("auth", &self.auth)
            .field("timeout", &self.timeout)
            .field("priority", &self.priority)
            .field("fallback_to_cached", &self.fallback_to_cached)
            .finish()
    }
}

impl HttpSourceBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            url: None,
            auth: HttpAuth::None,
            timeout: Duration::from_secs(10),
            priority: 250, // Higher than files (100-200), lower than env vars (300)
            fallback_to_cached: true,
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

    /// Enable or disable falling back to the cached last-known-good configuration
    /// when a subsequent HTTP fetch fails due to network or HTTP status errors.
    ///
    /// Enabled (`true`) by default. Initial fetches with no cached configuration
    /// and malformed JSON responses always return an error.
    pub fn with_fallback_to_cached(mut self, enabled: bool) -> Self {
        self.fallback_to_cached = enabled;
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

        let builder = Client::builder().timeout(self.timeout);

        // TLS backend selection. rustls is the default because it needs no
        // system libraries; `remote-native-tls` opts into the platform
        // certificate store instead.
        #[cfg(feature = "remote-native-tls")]
        let builder = builder.use_native_tls();
        #[cfg(not(feature = "remote-native-tls"))]
        let builder = builder.use_rustls_tls();

        let client = builder
            .build()
            .map_err(|e| ConfigError::LoadError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(HttpSource {
            url,
            client,
            auth: self.auth,
            priority: self.priority,
            fallback_to_cached: self.fallback_to_cached,
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
            } else if let Some(u) = n.as_u64() {
                Ok(config::Value::new(None, config::ValueKind::U64(u)))
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
    fn test_builder_and_debug_redaction() {
        let builder = HttpSource::builder()
            .with_url("https://admin:super_secret_pw@example.com/config")
            .with_auth_token("secret_bearer_token")
            .with_timeout(Duration::from_secs(5))
            .with_priority(200);

        let builder_dbg = format!("{:?}", builder);
        assert!(!builder_dbg.contains("super_secret_pw"));
        assert!(!builder_dbg.contains("secret_bearer_token"));
        assert!(builder_dbg.contains("<redacted>"));

        let source = builder.build().unwrap();
        assert_eq!(source.priority(), 200);

        let source_dbg = format!("{:?}", source);
        assert!(!source_dbg.contains("super_secret_pw"));
        assert!(!source_dbg.contains("secret_bearer_token"));
        assert!(!source.name().contains("super_secret_pw"));
    }

    #[test]
    fn test_builder_no_url() {
        let builder = HttpSource::builder();
        let source = builder.build();
        assert!(source.is_err());
    }

    #[test]
    fn test_builder_with_basic_auth_redaction() {
        let source = HttpSource::builder()
            .with_url("https://example.com/config")
            .with_basic_auth("user", "top_secret_pass")
            .build()
            .unwrap();

        let dbg = format!("{:?}", source);
        assert!(dbg.contains("user"));
        assert!(!dbg.contains("top_secret_pass"));
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn test_json_to_config_map_and_u64_precision() {
        use serde_json::json;

        let large_u64: u64 = (i64::MAX as u64) + 42;
        let json = json!({
            "server": {
                "port": 8080,
                "host": "localhost"
            },
            "max_bytes": large_u64,
            "debug": true
        });

        let map = json_to_config_map(json).unwrap();
        assert!(map.contains_key("server"));
        assert!(map.contains_key("debug"));
        assert_eq!(
            map.get("max_bytes")
                .and_then(|v| v.clone().into_uint().ok()),
            Some(large_u64)
        );
    }

    #[test]
    fn test_json_to_config_map_invalid() {
        use serde_json::json;

        let json = json!([1, 2, 3]); // Array at root, not object
        let map = json_to_config_map(json);
        assert!(map.is_err());
    }
}
