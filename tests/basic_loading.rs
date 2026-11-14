//! Integration tests for basic configuration loading, precedence, env overrides, and validation.

#![allow(unsafe_code)] // For env var manipulation in tests

use hotswap_config::error::{ConfigError, ValidationError};
use hotswap_config::prelude::*;
use serde::Deserialize;
use std::error::Error;
use std::fs;
use tempfile::TempDir;

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct ServerConfig {
    port: u16,
    host: String,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct DatabaseConfig {
    url: String,
    max_connections: u32,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct AppConfig {
    server: ServerConfig,
    database: DatabaseConfig,
}

#[cfg(feature = "validation")]
impl Validate for AppConfig {
    fn validate(&self) -> std::result::Result<(), ValidationError> {
        if self.server.port < 1024 {
            return Err(ValidationError::invalid_field(
                "server.port",
                "must be >= 1024",
            ));
        }
        if self.database.max_connections == 0 {
            return Err(ValidationError::invalid_field(
                "database.max_connections",
                "must be > 0",
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn test_load_single_yaml_file() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let config: HotswapConfig<AppConfig> = HotswapConfig::builder()
        .with_file(&config_path)
        .build::<AppConfig>()
        .await
        .unwrap();

    let cfg = config.get();
    assert_eq!(cfg.server.port, 8080);
    assert_eq!(cfg.server.host, "localhost");
    assert_eq!(cfg.database.url, "postgres://localhost/db");
    assert_eq!(cfg.database.max_connections, 10);
}

#[tokio::test]
async fn test_file_precedence() {
    let temp_dir = TempDir::new().unwrap();
    let default_path = temp_dir.path().join("default.yaml");
    let override_path = temp_dir.path().join("override.yaml");

    // Default config
    fs::write(
        &default_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    // Override config (only overrides server.port; sibling server.host must survive!)
    fs::write(
        &override_path,
        r#"
server:
  port: 9090
"#,
    )
    .unwrap();

    let config: HotswapConfig<AppConfig> = HotswapConfig::builder()
        .with_file(&default_path)
        .with_file(&override_path)
        .build::<AppConfig>()
        .await
        .unwrap();

    let cfg = config.get();
    assert_eq!(cfg.server.port, 9090); // Overridden
    assert_eq!(cfg.server.host, "localhost"); // Preserved from default
    assert_eq!(cfg.database.max_connections, 10); // Preserved from default
}

#[tokio::test]
async fn test_env_overrides() {
    use std::env;

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    unsafe {
        env::set_var("TEST_PHASE1_SERVER__PORT", "9999");
        env::set_var("TEST_PHASE1_DATABASE__MAX_CONNECTIONS", "50");
    }

    let build_res = HotswapConfig::builder()
        .with_file(&config_path)
        .with_env_overrides("TEST_PHASE1", "__")
        .build::<AppConfig>()
        .await;

    unsafe {
        env::remove_var("TEST_PHASE1_SERVER__PORT");
        env::remove_var("TEST_PHASE1_DATABASE__MAX_CONNECTIONS");
    }

    let config = build_res.unwrap();
    let cfg = config.get();
    assert_eq!(cfg.server.port, 9999); // From env
    assert_eq!(cfg.server.host, "localhost"); // Preserved from file
    assert_eq!(cfg.database.max_connections, 50); // From env
}

#[tokio::test]
async fn test_validation_success() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let result = HotswapConfig::builder()
        .with_file(&config_path)
        .with_validation(|config: &AppConfig| {
            if config.server.port < 1024 {
                return Err(ValidationError::invalid_field(
                    "server.port",
                    "must be >= 1024",
                ));
            }
            Ok(())
        })
        .build::<AppConfig>()
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_validation_failure() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 80
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let result = HotswapConfig::builder()
        .with_file(&config_path)
        .with_validation(|config: &AppConfig| {
            if config.server.port < 1024 {
                return Err(ValidationError::invalid_field(
                    "server.port",
                    "must be >= 1024",
                ));
            }
            Ok(())
        })
        .build::<AppConfig>()
        .await;

    let err = result.unwrap_err();
    assert!(err.to_string().contains("Configuration validation failed"));
    assert!(err.source().is_some());
}

#[cfg(feature = "validation")]
#[tokio::test]
async fn test_validate_trait_on_build_reload_and_update() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    // 1. Invalid initial config must fail `build_validated`
    fs::write(
        &config_path,
        r#"
server:
  port: 80
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let bad_build = HotswapConfig::builder()
        .with_file(&config_path)
        .build_validated::<AppConfig>()
        .await;
    assert!(matches!(bad_build, Err(ConfigError::ValidationError(_))));

    // 2. Valid initial config succeeds with `with_validate_trait` + `with_validation`
    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let config = HotswapConfig::builder()
        .with_file(&config_path)
        .with_validate_trait::<AppConfig>()
        .with_validation(|cfg: &AppConfig| {
            if cfg.server.host.is_empty() {
                return Err(ValidationError::invalid_field("server.host", "empty"));
            }
            Ok(())
        })
        .build::<AppConfig>()
        .await
        .unwrap();
    assert_eq!(config.get().server.port, 8080);

    // 3. Invalid reload is rejected by `Validate` trait and preserves old config
    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 0
"#,
    )
    .unwrap();

    let bad_reload = config.reload().await;
    assert!(matches!(bad_reload, Err(ConfigError::ValidationError(_))));
    assert_eq!(config.get().database.max_connections, 10);

    // 4. Invalid programmatic `update` is also rejected by `Validate` trait
    let bad_update = config
        .update(AppConfig {
            server: ServerConfig {
                port: 22,
                host: "localhost".to_string(),
            },
            database: DatabaseConfig {
                url: "postgres://localhost/db".to_string(),
                max_connections: 10,
            },
        })
        .await;
    assert!(matches!(bad_update, Err(ConfigError::ValidationError(_))));
    assert_eq!(config.get().server.port, 8080);
}

#[tokio::test]
async fn test_reload() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let config: HotswapConfig<AppConfig> = HotswapConfig::builder()
        .with_file(&config_path)
        .build::<AppConfig>()
        .await
        .unwrap();

    assert_eq!(config.get().server.port, 8080);

    fs::write(
        &config_path,
        r#"
server:
  port: 9090
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    config.reload().await.unwrap();
    assert_eq!(config.get().server.port, 9090);
}

#[tokio::test]
async fn test_manual_update() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let config: HotswapConfig<AppConfig> = HotswapConfig::builder()
        .with_file(&config_path)
        .build::<AppConfig>()
        .await
        .unwrap();

    let new_config = AppConfig {
        server: ServerConfig {
            port: 7777,
            host: "127.0.0.1".to_string(),
        },
        database: DatabaseConfig {
            url: "postgres://remote/db".to_string(),
            max_connections: 20,
        },
    };

    config.update(new_config.clone()).await.unwrap();
    assert_eq!(*config.get(), new_config);
}

#[tokio::test]
async fn test_clone_handle() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: localhost
database:
  url: postgres://localhost/db
  max_connections: 10
"#,
    )
    .unwrap();

    let config: HotswapConfig<AppConfig> = HotswapConfig::builder()
        .with_file(&config_path)
        .build::<AppConfig>()
        .await
        .unwrap();

    let config_clone = config.clone();
    assert_eq!(config.get().server.port, config_clone.get().server.port);

    let new_config = AppConfig {
        server: ServerConfig {
            port: 9090,
            host: "localhost".to_string(),
        },
        database: DatabaseConfig {
            url: "postgres://localhost/db".to_string(),
            max_connections: 10,
        },
    };
    config.update(new_config).await.unwrap();

    assert_eq!(config.get().server.port, 9090);
    assert_eq!(config_clone.get().server.port, 9090);
}
