//! File-based configuration source.

use super::ConfigSource;
use crate::error::{ConfigError, Result};
use config::File;
use std::collections::HashMap;
use std::path::PathBuf;

/// File-based configuration source.
///
/// Loads configuration from YAML, TOML, or JSON files with automatic format detection
/// based on file extension.
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::sources::FileSource;
///
/// let source = FileSource::new("config/default.yaml");
/// ```
pub struct FileSource {
    path: PathBuf,
    priority: i32,
}

impl FileSource {
    /// Create a new file source with automatic format detection.
    ///
    /// The format is detected from the file extension:
    /// - `.yaml`, `.yml` -> YAML
    /// - `.toml` -> TOML
    /// - `.json` -> JSON
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hotswap_config::sources::FileSource;
    ///
    /// let source = FileSource::new("config/default.yaml");
    /// ```
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            priority: 100,
        }
    }

    /// Set the priority for this source.
    ///
    /// Higher priority sources override lower priority ones.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Validate that the file extension is supported.
    fn validate_extension(&self) -> Result<()> {
        let extension = self
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            .ok_or_else(|| {
                ConfigError::LoadError(format!(
                    "Unable to determine file format for: {}",
                    self.path.display()
                ))
            })?;

        match extension {
            "yaml" | "yml" | "toml" | "json" => Ok(()),
            _ => Err(ConfigError::LoadError(format!(
                "Unsupported file extension: {}. Supported: .yaml, .yml, .toml, .json",
                extension
            ))),
        }
    }
}

impl ConfigSource for FileSource {
    fn load(&self) -> Result<HashMap<String, config::Value>> {
        // Validate extension
        self.validate_extension()?;

        // Check if file exists
        if !self.path.exists() {
            return Err(ConfigError::LoadError(format!(
                "Configuration file not found: {}",
                self.path.display()
            )));
        }

        // Build a config using the config crate (auto-detects format from extension)
        let config_builder = config::Config::builder()
            .add_source(File::from(self.path.clone()).required(true))
            .build()
            .map_err(|e| ConfigError::LoadError(format!("Failed to load file: {}", e)))?;

        // Extract as HashMap
        let map = config_builder
            .try_deserialize::<HashMap<String, config::Value>>()
            .map_err(|e| {
                ConfigError::DeserializationError(format!("Failed to parse file: {}", e))
            })?;

        Ok(map)
    }

    fn name(&self) -> String {
        format!("file:{}", self.path.display())
    }

    fn priority(&self) -> i32 {
        self.priority
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_validate_extension_yaml() {
        let source = FileSource::new("config.yaml");
        assert!(source.validate_extension().is_ok());

        let source = FileSource::new("config.yml");
        assert!(source.validate_extension().is_ok());
    }

    #[test]
    fn test_validate_extension_toml() {
        let source = FileSource::new("config.toml");
        assert!(source.validate_extension().is_ok());
    }

    #[test]
    fn test_validate_extension_json() {
        let source = FileSource::new("config.json");
        assert!(source.validate_extension().is_ok());
    }

    #[test]
    fn test_validate_extension_unknown() {
        let source = FileSource::new("config.txt");
        assert!(source.validate_extension().is_err());
    }

    // Needs a YAML parser, which since 0.2.1 arrives via the `yaml` feature
    // (on by default through `all-formats`) rather than unconditionally.
    #[cfg(feature = "yaml")]
    #[test]
    fn test_load_yaml_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");

        fs::write(
            &config_path,
            r#"
server:
  port: 8080
  host: localhost
"#,
        )
        .unwrap();

        let source = FileSource::new(&config_path);
        let result = source.load();
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let source = FileSource::new("/nonexistent/config.yaml");
        let result = source.load();
        assert!(result.is_err());
    }

    #[test]
    fn test_with_priority() {
        let source = FileSource::new("config.yaml").with_priority(200);
        assert_eq!(source.priority(), 200);
    }

    #[test]
    fn test_name() {
        let source = FileSource::new("config.yaml");
        assert!(source.name().contains("config.yaml"));
    }
}
