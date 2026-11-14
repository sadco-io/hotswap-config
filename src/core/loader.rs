//! Configuration loader that merges multiple sources.

use crate::error::{ConfigError, Result};
use crate::sources::ConfigSource;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fmt;

/// Internal `config::Source` adapter over an already deep-merged root map.
///
/// Using `config::Source::collect` rather than `ConfigBuilder::set_override` avoids
/// re-parsing map keys as dotted path expressions, preserving user map keys that
/// contain literal `.` characters.
#[derive(Clone, Debug)]
struct MergedMapSource(config::Map<String, config::Value>);

impl config::Source for MergedMapSource {
    fn clone_into_box(&self) -> Box<dyn config::Source + Send + Sync> {
        Box::new(self.clone())
    }

    fn collect(
        &self,
    ) -> std::result::Result<config::Map<String, config::Value>, config::ConfigError> {
        Ok(self.0.clone())
    }
}

/// Recursively merge `overlay` into `base`, preserving sibling keys inside nested tables.
fn merge_value(base: &mut config::Value, overlay: config::Value) {
    if matches!(
        (&base.kind, &overlay.kind),
        (config::ValueKind::Table(_), config::ValueKind::Table(_))
    ) {
        if let (config::ValueKind::Table(base_table), config::ValueKind::Table(overlay_table)) =
            (&mut base.kind, overlay.kind)
        {
            for (key, value) in overlay_table {
                match base_table.get_mut(&key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        base_table.insert(key, value);
                    }
                }
            }
        }
    } else {
        *base = overlay;
    }
}

/// Recursively merge `overlay` map entries into `base`.
pub(crate) fn deep_merge_maps(
    base: &mut HashMap<String, config::Value>,
    overlay: HashMap<String, config::Value>,
) {
    for (key, value) in overlay {
        match base.get_mut(&key) {
            Some(existing) => merge_value(existing, value),
            None => {
                base.insert(key, value);
            }
        }
    }
}

/// Loads and merges configuration from multiple sources.
///
/// The loader handles precedence by sorting sources by priority and deep-merging
/// them in order (lower priority first, higher priority sources override).
pub struct ConfigLoader {
    sources: Vec<Box<dyn ConfigSource>>,
}

impl fmt::Debug for ConfigLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigLoader")
            .field("sources", &self.source_names())
            .finish()
    }
}

impl ConfigLoader {
    /// Create a new configuration loader.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// Add a configuration source.
    pub fn add_source(&mut self, source: Box<dyn ConfigSource>) {
        self.sources.push(source);
    }

    /// Collect and deep-merge raw values from all configured sources without deserializing.
    ///
    /// Separated from [`Self::deserialize_merged`] so async callers (`build` and `reload`)
    /// can offload blocking source I/O onto `tokio::task::spawn_blocking` without requiring
    /// `T: Send + 'static`.
    pub(crate) fn collect_merged(&self) -> Result<HashMap<String, config::Value>> {
        if self.sources.is_empty() {
            return Err(ConfigError::LoadError(
                "No configuration sources specified".to_string(),
            ));
        }

        // Sort sources by priority (lowest first; stable sort preserves registration order on ties)
        let mut sorted_sources: Vec<_> = self.sources.iter().collect();
        sorted_sources.sort_by_key(|s| s.priority());

        let mut merged_values: HashMap<String, config::Value> = HashMap::new();

        // Deep-merge each source in priority order
        for source in sorted_sources {
            let values = source.load().map_err(|e| match e {
                ConfigError::FeatureNotEnabled(feat) => ConfigError::FeatureNotEnabled(feat),
                ConfigError::DeserializationError(msg) => ConfigError::DeserializationError(
                    format!("Failed to deserialize source '{}': {}", source.name(), msg),
                ),
                other => ConfigError::LoadError(format!(
                    "Failed to load source '{}': {}",
                    source.name(),
                    other
                )),
            })?;
            deep_merge_maps(&mut merged_values, values);
        }

        Ok(merged_values)
    }

    /// Deserialize an already deep-merged value map into `T`.
    pub(crate) fn deserialize_merged<T>(merged_values: HashMap<String, config::Value>) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let mut root_map = config::Map::new();
        for (k, v) in merged_values {
            root_map.insert(k, v);
        }

        let config = config::Config::builder()
            .add_source(MergedMapSource(root_map))
            .build()
            .map_err(|e| ConfigError::LoadError(format!("Failed to build configuration: {}", e)))?;

        config.try_deserialize::<T>().map_err(|e| {
            ConfigError::DeserializationError(format!("Failed to deserialize configuration: {}", e))
        })
    }

    /// Load and deep-merge configuration from all sources.
    ///
    /// Sources are merged in priority order (lowest to highest), so higher priority
    /// sources override leaf values from lower priority sources while preserving
    /// sibling fields in nested structs/maps.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The target configuration type
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Any source fails to load
    /// - Deserialization fails
    #[cfg(test)]
    pub fn load<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let merged_values = self.collect_merged()?;
        Self::deserialize_merged(merged_values)
    }

    /// Get the list of source names in priority order.
    pub fn source_names(&self) -> Vec<String> {
        let mut sorted_sources: Vec<_> = self.sources.iter().collect();
        sorted_sources.sort_by_key(|s| s.priority());
        sorted_sources.iter().map(|s| s.name()).collect()
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::ConfigSource;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct TestConfig {
        port: u16,
        host: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct NestedConfig {
        server: TestConfig,
        labels: HashMap<String, String>,
    }

    struct MockSource {
        name: String,
        priority: i32,
        values: HashMap<String, config::Value>,
    }

    impl MockSource {
        fn new(name: &str, priority: i32) -> Self {
            Self {
                name: name.to_string(),
                priority,
                values: HashMap::new(),
            }
        }

        fn with_value(mut self, key: &str, value: impl Into<config::Value>) -> Self {
            self.values.insert(key.to_string(), value.into());
            self
        }
    }

    impl ConfigSource for MockSource {
        fn load(&self) -> Result<HashMap<String, config::Value>> {
            Ok(self.values.clone())
        }

        fn name(&self) -> String {
            self.name.clone()
        }

        fn priority(&self) -> i32 {
            self.priority
        }
    }

    #[test]
    fn test_empty_loader() {
        let loader = ConfigLoader::new();
        let result: Result<TestConfig> = loader.load();
        assert!(result.is_err());
    }

    #[test]
    fn test_single_source() {
        let mut loader = ConfigLoader::new();
        let source = MockSource::new("test", 100)
            .with_value("port", 8080i64)
            .with_value("host", "localhost");
        loader.add_source(Box::new(source));

        let config: TestConfig = loader.load().unwrap();
        assert_eq!(config.port, 8080);
        assert_eq!(config.host, "localhost");
    }

    #[test]
    fn test_precedence() {
        let mut loader = ConfigLoader::new();

        let source1 = MockSource::new("default", 100)
            .with_value("port", 8080i64)
            .with_value("host", "localhost");

        let source2 = MockSource::new("override", 200).with_value("port", 9090i64);

        loader.add_source(Box::new(source1));
        loader.add_source(Box::new(source2));

        let config: TestConfig = loader.load().unwrap();
        assert_eq!(config.port, 9090);
        assert_eq!(config.host, "localhost");
    }

    #[test]
    fn test_deep_merge_nested_tables_and_dotted_map_keys() {
        let mut loader = ConfigLoader::new();

        let mut base_server = HashMap::new();
        base_server.insert("port".to_string(), config::Value::from(8080i64));
        base_server.insert("host".to_string(), config::Value::from("localhost"));

        let mut base_labels = HashMap::new();
        base_labels.insert(
            "app.kubernetes.io/name".to_string(),
            config::Value::from("api"),
        );

        let source1 = MockSource::new("base", 100)
            .with_value(
                "server",
                config::Value::new(None, config::ValueKind::Table(base_server)),
            )
            .with_value(
                "labels",
                config::Value::new(None, config::ValueKind::Table(base_labels)),
            );

        let mut override_server = HashMap::new();
        override_server.insert("port".to_string(), config::Value::from(9090i64));

        let mut override_labels = HashMap::new();
        override_labels.insert(
            "app.kubernetes.io/part-of".to_string(),
            config::Value::from("platform"),
        );

        let source2 = MockSource::new("override", 200)
            .with_value(
                "server",
                config::Value::new(None, config::ValueKind::Table(override_server)),
            )
            .with_value(
                "labels",
                config::Value::new(None, config::ValueKind::Table(override_labels)),
            );

        loader.add_source(Box::new(source1));
        loader.add_source(Box::new(source2));

        let config: NestedConfig = loader.load().unwrap();
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.server.host, "localhost");
        assert_eq!(
            config
                .labels
                .get("app.kubernetes.io/name")
                .map(String::as_str),
            Some("api")
        );
        assert_eq!(
            config
                .labels
                .get("app.kubernetes.io/part-of")
                .map(String::as_str),
            Some("platform")
        );
    }

    #[test]
    fn test_source_names() {
        let mut loader = ConfigLoader::new();
        loader.add_source(Box::new(MockSource::new("source1", 100)));
        loader.add_source(Box::new(MockSource::new("source2", 200)));
        loader.add_source(Box::new(MockSource::new("source3", 50)));

        let names = loader.source_names();
        assert_eq!(names, vec!["source3", "source1", "source2"]);
        assert!(format!("{:?}", loader).contains("ConfigLoader"));
    }
}
