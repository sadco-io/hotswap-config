//! Configuration source trait.

use crate::error::Result;
use std::collections::HashMap;

/// Trait for configuration sources.
///
/// Implement this trait to create custom configuration sources (e.g., remote APIs,
/// databases, key-value stores).
///
/// # Note
///
/// This is currently a synchronous trait. Async source support is planned for a future release.
pub trait ConfigSource: Send + Sync {
    /// Load configuration as a raw string key-value map.
    ///
    /// The returned map will be merged with other sources according to precedence rules.
    ///
    /// # Errors
    ///
    /// Returns an error if the source cannot be loaded or parsed.
    fn load(&self) -> Result<HashMap<String, config::Value>>;

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
