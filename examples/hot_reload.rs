//! Example demonstrating file watching and automatic hot-reload.
//!
//! This example shows how to:
//! - Enable file watching on configuration files
//! - Automatically reload when files change
//! - Subscribe to configuration change notifications
//!
//! Run with: cargo run --example hot_reload --features yaml
//!
//! While running, try editing examples/config/hot_reload.yaml to see automatic reloads.

use hotswap_config::prelude::*;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    server: ServerConfig,
    database: DatabaseConfig,
}

#[derive(Debug, Deserialize, Clone)]
struct ServerConfig {
    port: u16,
    host: String,
}

#[derive(Debug, Deserialize, Clone)]
struct DatabaseConfig {
    url: String,
    max_connections: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Hot Reload Example ===\n");

    // Create an initial config file if it doesn't exist
    let config_path = "examples/config/hot_reload.yaml";
    if !std::path::Path::new(config_path).exists() {
        std::fs::create_dir_all("examples/config")?;
        std::fs::write(
            config_path,
            r#"server:
  port: 8080
  host: localhost

database:
  url: postgres://localhost/mydb
  max_connections: 10
"#,
        )?;
        println!("Created {}", config_path);
    }

    // Build configuration with file watching enabled
    let config = HotswapConfig::builder()
        .with_file(config_path)
        .with_file_watch(true) // Enable automatic reloading
        .with_watch_debounce(std::time::Duration::from_millis(500)) // Debounce file changes
        .build::<AppConfig>()
        .await?;

    println!("Configuration loaded with file watching enabled");
    println!("Watching: {}\n", config_path);

    // Track the number of reloads
    let reload_count = Arc::new(AtomicUsize::new(0));
    let reload_count_clone = Arc::clone(&reload_count);

    // Subscribe to configuration changes
    let _subscription = config
        .subscribe(move || {
            let count = reload_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
            println!("\n[Event] Configuration reloaded (reload #{})", count);
        })
        .await;

    println!("Subscribed to configuration changes\n");

    // Print current configuration
    let cfg = config.get();
    println!("Current configuration:");
    println!("  Server: {}:{}", cfg.server.host, cfg.server.port);
    println!(
        "  Database: {} (max connections: {})",
        cfg.database.url, cfg.database.max_connections
    );

    println!(
        "\n===> Try editing {} to see automatic reloads! <===",
        config_path
    );
    println!("     Example changes:");
    println!("     - Change port: 8080 -> 9090");
    println!("     - Change host: localhost -> 0.0.0.0");
    println!("     - Change max_connections: 10 -> 20");
    println!("\nPress Ctrl+C to exit\n");

    // Keep the application running and periodically show the current config
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        let cfg = config.get();
        let count = reload_count.load(Ordering::SeqCst);

        println!("[Status] Config check (reloads: {}):", count);
        println!("  Server: {}:{}", cfg.server.host, cfg.server.port);
        println!(
            "  Database: {} (max: {})\n",
            cfg.database.url, cfg.database.max_connections
        );
    }
}
