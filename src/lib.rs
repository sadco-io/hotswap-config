//! # hotswap-config
//!
//! Zero-downtime configuration management with lock-free hot-reloads and atomic updates.
//!
//! ## Overview
//!
//! `hotswap-config` provides a production-ready configuration library that combines:
//! - Lock-free atomic reads using `arc-swap`
//! - Zero-downtime hot-reloads with validation
//! - Standard configuration precedence (files → env vars)
//! - Optional advanced features (partial updates, rollback, gradual rollout)
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use hotswap_config::prelude::*;
//! use serde::Deserialize;
//!
//! #[derive(Debug, Deserialize, Clone)]
//! struct AppConfig {
//!     server: ServerConfig,
//!     database: DatabaseConfig,
//! }
//!
//! #[derive(Debug, Deserialize, Clone)]
//! struct ServerConfig {
//!     port: u16,
//! }
//!
//! #[derive(Debug, Deserialize, Clone)]
//! struct DatabaseConfig {
//!     url: String,
//! }
//!
//! # async fn example() -> hotswap_config::error::Result<()> {
//! // Load configuration with standard precedence
//! let config = HotswapConfig::builder()
//!     .with_file("config/default.yaml")
//!     .with_env_overrides("APP", "__")
//!     .build::<AppConfig>()
//!     .await?;
//!
//! // Zero-cost reads (no locks!)
//! let cfg = config.get();
//! println!("Server port: {}", cfg.server.port);
//! # Ok(())
//! # }
//! ```
//!
//! ## Features
//!
//! - **Lock-free reads**: Sub-10ns read latency using `arc-swap`
//! - **Atomic updates**: Readers never see partial state
//! - **File watching**: Automatic reload on file changes
//! - **Validation**: Reject invalid configs, keep old one
//! - **Partial updates**: JSON Patch for surgical changes
//! - **Rollback**: Time-travel to previous configs
//! - **Gradual rollout**: A/B test configuration changes
//! - **Remote sources**: HTTP endpoint support
//!
//! ## Feature Flags
//!
//! Enable optional features in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! hotswap-config = { version = "0.2", features = ["partial-updates", "rollback"] }
//! ```
//!
//! See the [crate documentation](https://docs.rs/hotswap-config) for all available features.

#![warn(missing_docs, rust_2024_compatibility)]
#![deny(unsafe_code)]

pub mod core;
pub mod error;
pub mod sources;

#[cfg(feature = "partial-updates")]
pub mod features;

#[cfg(feature = "file-watch")]
pub mod notify;

#[cfg(feature = "metrics")]
pub mod metrics;

/// Convenient re-exports for common usage patterns.
pub mod prelude {
    pub use crate::core::{HotswapConfig, HotswapConfigBuilder};
    pub use crate::error::{ConfigError, Result, ValidationError};

    #[cfg(feature = "validation")]
    pub use crate::core::Validate;
}
