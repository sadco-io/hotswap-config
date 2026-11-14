//! File-based configuration source.

use super::ConfigSource;
use crate::error::{ConfigError, Result};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetectedFormat {
    Yaml,
    Toml,
    Json,
}

/// File-based configuration source.
///
/// Loads configuration from YAML, TOML, or JSON files with automatic format detection
/// based on file extension (case-insensitive).
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::sources::FileSource;
///
/// let source = FileSource::new("config/default.yaml");
/// ```
#[derive(Debug, Clone)]
pub struct FileSource {
    path: PathBuf,
    priority: i32,
}

impl FileSource {
    /// Create a new file source with automatic format detection.
    ///
    /// The format is detected case-insensitively from the file extension:
    /// - `.yaml`, `.yml` -> YAML (requires `yaml` feature)
    /// - `.toml` -> TOML (requires `toml` feature)
    /// - `.json` -> JSON (requires `json` feature)
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

    /// Validate that the file extension is supported and return the detected format.
    fn validate_extension(&self) -> Result<DetectedFormat> {
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

        match extension.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => Ok(DetectedFormat::Yaml),
            "toml" => Ok(DetectedFormat::Toml),
            "json" => Ok(DetectedFormat::Json),
            _ => Err(ConfigError::LoadError(format!(
                "Unsupported file extension: {}. Supported: .yaml, .yml, .toml, .json",
                extension
            ))),
        }
    }

    /// Resolve `DetectedFormat` to `config::FileFormat`, returning `FeatureNotEnabled`
    /// when the corresponding parser feature flag is disabled.
    fn resolve_file_format(format: DetectedFormat) -> Result<config::FileFormat> {
        match format {
            DetectedFormat::Yaml => {
                #[cfg(feature = "yaml")]
                {
                    Ok(config::FileFormat::Yaml)
                }
                #[cfg(not(feature = "yaml"))]
                {
                    Err(ConfigError::FeatureNotEnabled("yaml"))
                }
            }
            DetectedFormat::Toml => {
                #[cfg(feature = "toml")]
                {
                    Ok(config::FileFormat::Toml)
                }
                #[cfg(not(feature = "toml"))]
                {
                    Err(ConfigError::FeatureNotEnabled("toml"))
                }
            }
            DetectedFormat::Json => {
                #[cfg(feature = "json")]
                {
                    Ok(config::FileFormat::Json)
                }
                #[cfg(not(feature = "json"))]
                {
                    Err(ConfigError::FeatureNotEnabled("json"))
                }
            }
        }
    }
}

impl ConfigSource for FileSource {
    fn load(&self) -> Result<HashMap<String, config::Value>> {
        // Validate extension and ensure the corresponding format feature is enabled
        let detected = self.validate_extension()?;
        let file_format = Self::resolve_file_format(detected)?;

        // Check if file exists
        if !self.path.exists() {
            return Err(ConfigError::LoadError(format!(
                "Configuration file not found: {}",
                self.path.display()
            )));
        }

        // Build a config using the config crate with explicit format (supports uppercase extensions)
        let config_builder = config::Config::builder()
            .add_source(
                config::File::from(self.path.clone())
                    .format(file_format)
                    .required(true),
            )
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

    #[cfg(feature = "yaml")]
    use std::fs;
    #[cfg(feature = "yaml")]
    use tempfile::TempDir;

    #[test]
    fn test_validate_extension_yaml() {
        let source = FileSource::new("config.yaml");
        assert_eq!(source.validate_extension().unwrap(), DetectedFormat::Yaml);

        let source = FileSource::new("config.yml");
        assert_eq!(source.validate_extension().unwrap(), DetectedFormat::Yaml);

        let source_upper = FileSource::new("CONFIG.YAML");
        assert_eq!(
            source_upper.validate_extension().unwrap(),
            DetectedFormat::Yaml
        );
    }

    #[test]
    fn test_validate_extension_toml() {
        let source = FileSource::new("config.toml");
        assert_eq!(source.validate_extension().unwrap(), DetectedFormat::Toml);

        let source_upper = FileSource::new("Config.TOML");
        assert_eq!(
            source_upper.validate_extension().unwrap(),
            DetectedFormat::Toml
        );
    }

    #[test]
    fn test_validate_extension_json() {
        let source = FileSource::new("config.json");
        assert_eq!(source.validate_extension().unwrap(), DetectedFormat::Json);

        let source_upper = FileSource::new("app.JSON");
        assert_eq!(
            source_upper.validate_extension().unwrap(),
            DetectedFormat::Json
        );
    }

    #[test]
    fn test_validate_extension_unknown() {
        let source = FileSource::new("config.txt");
        assert!(source.validate_extension().is_err());
    }

    #[cfg(not(feature = "yaml"))]
    #[test]
    fn test_yaml_feature_not_enabled() {
        let source = FileSource::new("config.yaml");
        assert!(matches!(
            source.load(),
            Err(ConfigError::FeatureNotEnabled("yaml"))
        ));
    }

    #[cfg(not(feature = "toml"))]
    #[test]
    fn test_toml_feature_not_enabled() {
        let source = FileSource::new("config.toml");
        assert!(matches!(
            source.load(),
            Err(ConfigError::FeatureNotEnabled("toml"))
        ));
    }

    #[cfg(not(feature = "json"))]
    #[test]
    fn test_json_feature_not_enabled() {
        let source = FileSource::new("config.json");
        assert!(matches!(
            source.load(),
            Err(ConfigError::FeatureNotEnabled("json"))
        ));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn test_load_yaml_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("CONFIG.YAML");

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
    fn test_with_priority_and_debug() {
        let source = FileSource::new("config.yaml").with_priority(200);
        assert_eq!(source.priority(), 200);
        assert!(format!("{:?}", source).contains("FileSource"));
    }

    #[test]
    fn test_name() {
        let source = FileSource::new("config.yaml");
        assert!(source.name().contains("config.yaml"));
    }
}
