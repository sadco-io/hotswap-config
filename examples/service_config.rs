//! Comprehensive example of a realistic microservice configuration.
//!
//! This example demonstrates:
//! - Defining service-specific configuration structs
//! - Nested configuration with multiple types
//! - Validation rules for business logic
//! - Environment variable overrides
//! - Configuration precedence (env vars > files)
//! - File watching for automatic reloads
//!
//! Run with: cargo run --example service_config --features yaml

use hotswap_config::prelude::*;
use serde::Deserialize;
use std::time::Duration;

/// Top-level service configuration
#[derive(Debug, Deserialize, Clone)]
struct ServiceConfig {
    /// Application metadata
    app: AppMetadata,

    /// HTTP server configuration
    server: ServerConfig,

    /// Database connection settings
    database: DatabaseConfig,

    /// Redis cache settings
    cache: CacheConfig,

    /// Security and authentication
    security: SecurityConfig,

    /// Feature flags for gradual rollout
    features: FeatureFlags,

    /// Observability settings
    observability: ObservabilityConfig,
}

#[derive(Debug, Deserialize, Clone)]
struct AppMetadata {
    name: String,
    version: String,
    environment: String, // dev, staging, production
}

#[derive(Debug, Deserialize, Clone)]
struct ServerConfig {
    host: String,
    port: u16,
    #[serde(default = "default_max_connections")]
    max_connections: usize,
    #[serde(default = "default_timeout_seconds")]
    timeout_seconds: u64,
    #[serde(default)]
    enable_cors: bool,
}

#[derive(Deserialize, Clone)]
struct DatabaseConfig {
    host: String,
    port: u16,
    database: String,
    username: String,
    #[allow(dead_code)] // Used for connection string construction
    password: String,
    #[serde(default = "default_pool_size")]
    pool_size: u32,
    #[serde(default = "default_connection_timeout")]
    connection_timeout_ms: u64,
}

impl std::fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("pool_size", &self.pool_size)
            .field("connection_timeout_ms", &self.connection_timeout_ms)
            .finish()
    }
}

#[derive(Debug, Deserialize, Clone)]
struct CacheConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_redis_url")]
    redis_url: String,
    #[serde(default = "default_ttl_seconds")]
    ttl_seconds: u64,
}

#[derive(Deserialize, Clone)]
struct SecurityConfig {
    jwt_secret: String,
    #[serde(default = "default_token_expiry")]
    token_expiry_hours: u64,
    #[serde(default)]
    require_https: bool,
    #[serde(default)]
    allowed_origins: Vec<String>,
}

impl std::fmt::Debug for SecurityConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityConfig")
            .field("jwt_secret", &"[REDACTED]")
            .field("token_expiry_hours", &self.token_expiry_hours)
            .field("require_https", &self.require_https)
            .field("allowed_origins", &self.allowed_origins)
            .finish()
    }
}

#[derive(Debug, Deserialize, Clone)]
struct FeatureFlags {
    #[serde(default)]
    new_api_enabled: bool,
    #[serde(default)]
    beta_features: bool,
    #[serde(default)]
    maintenance_mode: bool,
}

#[derive(Debug, Deserialize, Clone)]
struct ObservabilityConfig {
    #[serde(default = "default_log_level")]
    log_level: String,
    #[serde(default)]
    metrics_enabled: bool,
    #[serde(default)]
    tracing_enabled: bool,
    #[serde(default = "default_metrics_port")]
    metrics_port: u16,
}

// Default value functions
fn default_max_connections() -> usize {
    1000
}
fn default_timeout_seconds() -> u64 {
    30
}
fn default_pool_size() -> u32 {
    10
}
fn default_connection_timeout() -> u64 {
    5000
}
fn default_redis_url() -> String {
    "redis://localhost:6379".to_string()
}
fn default_ttl_seconds() -> u64 {
    300
}
fn default_token_expiry() -> u64 {
    24
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_metrics_port() -> u16 {
    9090
}

/// Validation function for service configuration
fn validate_service_config(config: &ServiceConfig) -> std::result::Result<(), ValidationError> {
    // Validate server port is not privileged (< 1024) in production
    if config.app.environment == "production" && config.server.port < 1024 {
        return Err(ValidationError::invalid_field(
            "server.port",
            "production services must use non-privileged ports (>= 1024)",
        ));
    }

    // Validate database configuration
    if config.database.pool_size == 0 {
        return Err(ValidationError::invalid_field(
            "database.pool_size",
            "pool size must be greater than 0",
        ));
    }

    if config.database.pool_size > 100 {
        return Err(ValidationError::invalid_field(
            "database.pool_size",
            "pool size must not exceed 100 for stability",
        ));
    }

    // Validate security settings in production
    if config.app.environment == "production" {
        if config.security.jwt_secret.len() < 32 {
            return Err(ValidationError::invalid_field(
                "security.jwt_secret",
                "JWT secret must be at least 32 characters in production",
            ));
        }

        if !config.security.require_https {
            return Err(ValidationError::invalid_field(
                "security.require_https",
                "HTTPS must be required in production",
            ));
        }
    }

    // Validate observability settings
    let valid_log_levels = ["trace", "debug", "info", "warn", "error"];
    if !valid_log_levels.contains(&config.observability.log_level.as_str()) {
        return Err(ValidationError::invalid_field(
            "observability.log_level",
            format!("log level must be one of: {}", valid_log_levels.join(", ")),
        ));
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Service Configuration Example ===\n");

    // Create example config file if it doesn't exist
    let config_path = "examples/config/service.yaml";
    if !std::path::Path::new(config_path).exists() {
        std::fs::create_dir_all("examples/config")?;
        std::fs::write(
            config_path,
            r#"app:
  name: my-service
  version: 1.0.0
  environment: development

server:
  host: 0.0.0.0
  port: 8080
  max_connections: 1000
  timeout_seconds: 30
  enable_cors: true

database:
  host: localhost
  port: 5432
  database: myapp_db
  username: postgres
  password: dev_password
  pool_size: 10
  connection_timeout_ms: 5000

cache:
  enabled: true
  redis_url: redis://localhost:6379
  ttl_seconds: 300

security:
  jwt_secret: dev_secret_key_change_in_production_abc123
  token_expiry_hours: 24
  require_https: false
  allowed_origins:
    - http://localhost:3000
    - http://localhost:8080

features:
  new_api_enabled: false
  beta_features: false
  maintenance_mode: false

observability:
  log_level: info
  metrics_enabled: true
  tracing_enabled: true
  metrics_port: 9090
"#,
        )?;
        println!("Created example config: {}", config_path);
    }

    println!("Loading configuration with validation...\n");

    // Build configuration with all features
    let config = HotswapConfig::builder()
        // 1. Load from default file (priority: 100)
        .with_file(config_path)
        // 2. Override with environment variables (priority: 300)
        // Example: APP_SERVER__PORT=9000 will override server.port
        .with_env_overrides("APP", "__")
        // 3. Enable file watching for automatic reloads
        .with_file_watch(true)
        .with_watch_debounce(Duration::from_millis(500))
        // 4. Add validation logic
        .with_validation(validate_service_config)
        // Build the configuration
        .build::<ServiceConfig>()
        .await?;

    println!("✓ Configuration loaded and validated successfully\n");

    // Get the current configuration (wait-free read!)
    let cfg = config.get();

    // Display the loaded configuration
    println!("=== Loaded Configuration ===\n");

    println!("Application:");
    println!("  Name:        {}", cfg.app.name);
    println!("  Version:     {}", cfg.app.version);
    println!("  Environment: {}", cfg.app.environment);

    println!("\nServer:");
    println!("  Address:     {}:{}", cfg.server.host, cfg.server.port);
    println!("  Max Conns:   {}", cfg.server.max_connections);
    println!("  Timeout:     {}s", cfg.server.timeout_seconds);
    println!("  CORS:        {}", cfg.server.enable_cors);

    println!("\nDatabase:");
    println!("  Host:        {}:{}", cfg.database.host, cfg.database.port);
    println!("  Database:    {}", cfg.database.database);
    println!("  Username:    {}", cfg.database.username);
    println!("  Pool Size:   {}", cfg.database.pool_size);
    println!("  Timeout:     {}ms", cfg.database.connection_timeout_ms);

    println!("\nCache:");
    println!("  Enabled:     {}", cfg.cache.enabled);
    println!("  Redis URL:   {}", cfg.cache.redis_url);
    println!("  TTL:         {}s", cfg.cache.ttl_seconds);

    println!("\nSecurity:");
    println!("  JWT Secret:  [REDACTED]");
    println!("  Token TTL:   {}h", cfg.security.token_expiry_hours);
    println!("  HTTPS:       {}", cfg.security.require_https);
    println!("  Origins:     {:?}", cfg.security.allowed_origins);

    println!("\nFeature Flags:");
    println!("  New API:     {}", cfg.features.new_api_enabled);
    println!("  Beta:        {}", cfg.features.beta_features);
    println!("  Maintenance: {}", cfg.features.maintenance_mode);

    println!("\nObservability:");
    println!("  Log Level:   {}", cfg.observability.log_level);
    println!(
        "  Metrics:     {} (port {})",
        cfg.observability.metrics_enabled, cfg.observability.metrics_port
    );
    println!("  Tracing:     {}", cfg.observability.tracing_enabled);

    println!("\n=== Usage Examples ===\n");

    println!("1. Environment Variable Override:");
    println!("   export APP_SERVER__PORT=9000");
    println!("   export APP_FEATURES__BETA_FEATURES=true");
    println!("   cargo run --example service_config --features yaml");

    println!("\n2. Hot Reload:");
    println!("   Edit {} while this is running", config_path);
    println!("   Changes will be automatically reloaded");

    println!("\n3. Accessing Config in Your Service:");
    println!("   let cfg = config.get();  // Zero-latency read!");
    println!("   let port = cfg.server.port;");
    println!("   let db_url = format!(\"postgres://{{}}:{{}}@{{}}:{{}}/{{}}\",");
    println!("       cfg.database.username, cfg.database.password,");
    println!("       cfg.database.host, cfg.database.port, cfg.database.database);");

    println!("\n4. React to Config Changes:");
    println!("   config.subscribe(|| {{");
    println!("       println!(\"Config updated!\");");
    println!("       // Reconfigure services, update feature flags, etc.");
    println!("   }}).await;");

    // Subscribe to changes
    let _sub = config
        .subscribe(|| {
            println!("\n🔄 Configuration reloaded!");
        })
        .await;

    println!("\n=== Watching for changes (Ctrl+C to exit) ===\n");

    // Keep running to demonstrate hot reload
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let cfg = config.get();
        println!(
            "[Status] Service running on {}:{} (env: {})",
            cfg.server.host, cfg.server.port, cfg.app.environment
        );
    }
}
