//! Configuration validation support.

use crate::error::ValidationError;

/// Trait for configuration validation.
///
/// Implement this trait on your configuration types to enable automatic validation
/// on initial load, reloads, and programmatic updates via
/// [`HotswapConfigBuilder::build_validated`](crate::core::HotswapConfigBuilder::build_validated)
/// or [`HotswapConfigBuilder::with_validate_trait`](crate::core::HotswapConfigBuilder::with_validate_trait).
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::prelude::*;
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize, Clone)]
/// struct ServerConfig {
///     port: u16,
///     max_connections: usize,
/// }
///
/// impl Validate for ServerConfig {
///     fn validate(&self) -> std::result::Result<(), ValidationError> {
///         if self.port < 1024 {
///             return Err(ValidationError::invalid_field(
///                 "port",
///                 "must be >= 1024 (privileged ports require root)"
///             ));
///         }
///
///         if self.max_connections == 0 {
///             return Err(ValidationError::invalid_field(
///                 "max_connections",
///                 "must be greater than 0"
///             ));
///         }
///
///         Ok(())
///     }
/// }
///
/// # async fn example() -> Result<()> {
/// // Build and automatically run `ServerConfig::validate` on build, reload, and update:
/// let config = HotswapConfig::builder()
///     .with_file("config/server.yaml")
///     .build_validated::<ServerConfig>()
///     .await?;
///
/// // Equivalent explicit registration:
/// let _builder = HotswapConfig::builder()
///     .with_file("config/server.yaml")
///     .with_validate_trait::<ServerConfig>();
/// # let _ = config;
/// # Ok(())
/// # }
/// ```
pub trait Validate {
    /// Validate the configuration.
    ///
    /// # Errors
    ///
    /// Should return a `ValidationError` describing what validation failed.
    fn validate(&self) -> Result<(), ValidationError>;
}
