//! Error types for hotswap-config.

use std::fmt;

/// Result type alias for hotswap-config operations.
pub type Result<T> = std::result::Result<T, ConfigError>;

/// Errors that can occur when working with configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Failed to load configuration from a source.
    #[error("Failed to load configuration: {0}")]
    LoadError(String),

    /// Failed to deserialize configuration.
    #[error("Failed to deserialize configuration: {0}")]
    DeserializationError(String),

    /// Configuration validation failed.
    #[error("Configuration validation failed: {0}")]
    ValidationError(#[from] ValidationError),

    /// File watching is not supported or failed to initialize.
    #[error("File watching error: {0}")]
    WatchError(String),

    /// Attempted to use a feature that is not enabled.
    #[error("Feature not enabled: {0}")]
    FeatureNotEnabled(&'static str),

    /// Configuration source does not support watching.
    #[error("Configuration source does not support watching")]
    WatchNotSupported,

    /// IO error occurred.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Failed to parse configuration file.
    #[error("Failed to parse configuration: {0}")]
    ParseError(String),

    #[cfg(feature = "rollback")]
    /// Not enough history to rollback the requested number of steps.
    #[error("Insufficient history: cannot rollback {requested} steps (only {available} available)")]
    InsufficientHistory {
        /// Number of steps requested to roll back
        requested: usize,
        /// Number of historical versions available
        available: usize,
    },

    #[cfg(feature = "partial-updates")]
    /// JSON patch operation failed.
    #[error("Patch operation failed: {0}")]
    PatchError(String),

    /// Generic error for other cases.
    #[error("Configuration error: {0}")]
    Other(String),
}

/// Validation error for configuration validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// Custom validation error with a message.
    Custom(String),

    /// A specific field has an invalid value.
    InvalidField {
        /// The field name/path
        field: String,
        /// The reason why it's invalid
        reason: String,
    },

    /// Multiple validation errors occurred.
    Multiple(Vec<ValidationError>),
}

impl ValidationError {
    /// Create a custom validation error.
    pub fn custom(msg: impl Into<String>) -> Self {
        Self::Custom(msg.into())
    }

    /// Create an invalid field error.
    pub fn invalid_field(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidField {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Custom(msg) => write!(f, "{}", msg),
            Self::InvalidField { field, reason } => {
                write!(f, "Field '{}' is invalid: {}", field, reason)
            }
            Self::Multiple(errors) => {
                write!(f, "Multiple validation errors:")?;
                for (i, err) in errors.iter().enumerate() {
                    write!(f, "\n  {}. {}", i + 1, err)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_validation_error_source_preserved() {
        let val_err = ValidationError::invalid_field("port", "must be >= 1024");
        let cfg_err: ConfigError = val_err.clone().into();
        assert_eq!(
            cfg_err.to_string(),
            "Configuration validation failed: Field 'port' is invalid: must be >= 1024"
        );
        let source = cfg_err
            .source()
            .expect("ValidationError should be preserved as Error::source");
        let downcast = source
            .downcast_ref::<ValidationError>()
            .expect("source should downcast to ValidationError");
        assert_eq!(downcast, &val_err);
    }

    #[test]
    fn test_multiple_validation_errors_no_trailing_newline() {
        let err = ValidationError::Multiple(vec![
            ValidationError::custom("first issue"),
            ValidationError::invalid_field("host", "cannot be empty"),
        ]);
        let formatted = err.to_string();
        assert!(!formatted.ends_with('\n'));
        assert_eq!(
            formatted,
            "Multiple validation errors:\n  1. first issue\n  2. Field 'host' is invalid: cannot be empty"
        );
    }
}
