//! Full integration tests exercising file watching, atomic saves, drop cleanup,
//! subscribers, metrics, rollback, and gradual rollout together.

use hotswap_config::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tempfile::TempDir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct IntegrationConfig {
    server: ServerConfig,
    database: DatabaseConfig,
    features: Features,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ServerConfig {
    port: u16,
    host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct DatabaseConfig {
    url: String,
    max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Features {
    enable_metrics: bool,
    enable_caching: bool,
}

#[tokio::test]
async fn test_file_loading_basic() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: "localhost"

database:
  url: "postgresql://localhost/mydb"
  max_connections: 10

features:
  enable_metrics: false
  enable_caching: true
"#,
    )
    .unwrap();

    let config = HotswapConfig::builder()
        .with_file(&config_path)
        .build::<IntegrationConfig>()
        .await
        .unwrap();

    let cfg = config.get();
    assert_eq!(cfg.server.port, 8080);
    assert_eq!(cfg.server.host, "localhost");
    assert_eq!(cfg.database.max_connections, 10);
    assert!(!cfg.features.enable_metrics);
    assert!(cfg.features.enable_caching);
}

#[tokio::test]
async fn test_file_watching_atomic_rename_and_clean_drop_shutdown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");
    let temp_save_path = temp_dir.path().join(".config.yaml.tmp");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: "localhost"

database:
  url: "postgresql://localhost/mydb"
  max_connections: 10

features:
  enable_metrics: false
  enable_caching: true
"#,
    )
    .unwrap();

    let notify_count = Arc::new(AtomicUsize::new(0));
    let notify_clone = Arc::clone(&notify_count);

    let config = HotswapConfig::builder()
        .with_file(&config_path)
        .with_validation(|cfg: &IntegrationConfig| {
            if cfg.server.port < 1024 {
                return Err(ValidationError::invalid_field("port", "must be >= 1024"));
            }
            Ok(())
        })
        .with_file_watch(true)
        .with_watch_debounce(Duration::from_millis(80))
        .build::<IntegrationConfig>()
        .await
        .unwrap();

    assert!(config.is_watching());
    let weak_inner = Arc::downgrade(&config.get());

    let _sub = config
        .subscribe(move || {
            notify_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    // Atomic rename save (`vim` / `sed -i` pattern) triggers automatic reload!
    fs::write(
        &temp_save_path,
        r#"
server:
  port: 9090
  host: "0.0.0.0"

database:
  url: "postgresql://localhost/mydb"
  max_connections: 20

features:
  enable_metrics: true
  enable_caching: true
"#,
    )
    .unwrap();
    fs::rename(&temp_save_path, &config_path).unwrap();

    // Poll until auto-reload updates port to 9090
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while config.get().server.port != 9090 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let cfg = config.get();
    assert_eq!(cfg.server.port, 9090);
    assert_eq!(cfg.server.host, "0.0.0.0");
    assert_eq!(cfg.database.max_connections, 20);
    assert!(cfg.features.enable_metrics);
    assert!(
        weak_inner.upgrade().is_none(),
        "Old config Arc should be freed"
    );
    let count_before_drop = notify_count.load(Ordering::SeqCst);
    assert!(count_before_drop >= 1);

    // Drop `cfg` and `config` (while keeping `_sub` to observe if any leaked task fires)
    drop(cfg);
    drop(config);

    // Give background tasks a moment to observe channel closure and exit
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Modify file after `HotswapConfig` dropped — no leaked reload task should run!
    fs::write(
        &config_path,
        r#"
server:
  port: 9191
  host: "0.0.0.0"

database:
  url: "postgresql://localhost/mydb"
  max_connections: 30

features:
  enable_metrics: true
  enable_caching: true
"#,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(notify_count.load(Ordering::SeqCst), count_before_drop);
}

#[tokio::test]
async fn test_subscribers_notification_and_immediate_unsubscribe() {
    let config = HotswapConfig::new(IntegrationConfig {
        server: ServerConfig {
            port: 8080,
            host: "localhost".to_string(),
        },
        database: DatabaseConfig {
            url: "postgresql://localhost/mydb".to_string(),
            max_connections: 10,
        },
        features: Features {
            enable_metrics: false,
            enable_caching: true,
        },
    });

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let handle = config
        .subscribe(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    let new_config = IntegrationConfig {
        server: ServerConfig {
            port: 9090,
            host: "0.0.0.0".to_string(),
        },
        database: DatabaseConfig {
            url: "postgresql://localhost/mydb".to_string(),
            max_connections: 20,
        },
        features: Features {
            enable_metrics: true,
            enable_caching: true,
        },
    };

    config.update(new_config.clone()).await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Immediate unsubscription with zero sleep
    drop(handle);
    config.update(new_config).await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "metrics")]
#[tokio::test]
async fn test_metrics_integration_and_subscriber_hook() {
    use opentelemetry::global;

    let meter = global::meter("test");

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: "localhost"

database:
  url: "postgresql://localhost/mydb"
  max_connections: 10

features:
  enable_metrics: false
  enable_caching: true
"#,
    )
    .unwrap();

    let config = HotswapConfig::builder()
        .with_file(&config_path)
        .with_metrics(meter)
        .build::<IntegrationConfig>()
        .await
        .unwrap();

    assert!(config.metrics().is_some());
    config.metrics().unwrap().update_config_age();

    let sub = config.subscribe(|| {}).await;
    config.reload().await.unwrap();
    drop(sub);

    let new_config = IntegrationConfig {
        server: ServerConfig {
            port: 9090,
            host: "0.0.0.0".to_string(),
        },
        database: DatabaseConfig {
            url: "postgresql://localhost/mydb".to_string(),
            max_connections: 20,
        },
        features: Features {
            enable_metrics: true,
            enable_caching: true,
        },
    };

    config.update(new_config).await.unwrap();
}

#[cfg(feature = "rollback")]
#[tokio::test]
async fn test_reload_auto_records_rollback_history() {
    use hotswap_config::features::Rollback;

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: "localhost"
database:
  url: "postgresql://localhost/mydb"
  max_connections: 10
features:
  enable_metrics: false
  enable_caching: true
"#,
    )
    .unwrap();

    let config = HotswapConfig::builder()
        .with_file(&config_path)
        .build::<IntegrationConfig>()
        .await
        .unwrap();

    let history = config.enable_history(10);
    assert_eq!(history.len().await, 1);

    fs::write(
        &config_path,
        r#"
server:
  port: 8081
  host: "localhost"
database:
  url: "postgresql://localhost/mydb"
  max_connections: 10
features:
  enable_metrics: false
  enable_caching: true
"#,
    )
    .unwrap();
    config.reload().await.unwrap();
    assert_eq!(history.len().await, 2);

    fs::write(
        &config_path,
        r#"
server:
  port: 8082
  host: "localhost"
database:
  url: "postgresql://localhost/mydb"
  max_connections: 10
features:
  enable_metrics: false
  enable_caching: true
"#,
    )
    .unwrap();
    config.reload().await.unwrap();
    assert_eq!(history.len().await, 3);
    assert_eq!(config.get().server.port, 8082);

    config.rollback(&history, 1).await.unwrap();
    assert_eq!(config.get().server.port, 8081);

    config.rollback(&history, 1).await.unwrap();
    assert_eq!(config.get().server.port, 8080);
}

#[tokio::test]
async fn test_validation_failure_preserves_old_config() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.yaml");

    fs::write(
        &config_path,
        r#"
server:
  port: 8080
  host: "localhost"

database:
  url: "postgresql://localhost/mydb"
  max_connections: 10

features:
  enable_metrics: false
  enable_caching: true
"#,
    )
    .unwrap();

    let config_with_validation = HotswapConfig::builder()
        .with_file(&config_path)
        .with_validation(|cfg: &IntegrationConfig| {
            if cfg.server.port < 1024 {
                return Err(ValidationError::invalid_field("port", "must be >= 1024"));
            }
            Ok(())
        })
        .build::<IntegrationConfig>()
        .await
        .unwrap();

    let invalid_config = IntegrationConfig {
        server: ServerConfig {
            port: 80,
            host: "localhost".to_string(),
        },
        database: DatabaseConfig {
            url: "postgresql://localhost/mydb".to_string(),
            max_connections: 10,
        },
        features: Features {
            enable_metrics: false,
            enable_caching: true,
        },
    };

    let result = config_with_validation.update(invalid_config).await;
    assert!(result.is_err());

    let cfg = config_with_validation.get();
    assert_eq!(cfg.server.port, 8080);
}

#[tokio::test]
async fn test_concurrent_reads_during_updates() {
    let config = Arc::new(HotswapConfig::new(IntegrationConfig {
        server: ServerConfig {
            port: 8080,
            host: "localhost".to_string(),
        },
        database: DatabaseConfig {
            url: "postgresql://localhost/mydb".to_string(),
            max_connections: 10,
        },
        features: Features {
            enable_metrics: false,
            enable_caching: true,
        },
    }));

    let mut handles = vec![];

    for _ in 0..10 {
        let cfg = config.clone();
        let handle = tokio::spawn(async move {
            for _ in 0..100 {
                let data = cfg.get();
                assert!(data.server.port >= 8080 && data.server.port <= 8090);
                tokio::time::sleep(Duration::from_micros(10)).await;
            }
        });
        handles.push(handle);
    }

    for i in 0..10 {
        let new_config = IntegrationConfig {
            server: ServerConfig {
                port: 8080 + i,
                host: "localhost".to_string(),
            },
            database: DatabaseConfig {
                url: "postgresql://localhost/mydb".to_string(),
                max_connections: 10,
            },
            features: Features {
                enable_metrics: false,
                enable_caching: true,
            },
        };

        config.update(new_config).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    for handle in handles {
        handle.await.unwrap();
    }
}
