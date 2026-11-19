//! Example demonstrating gradual configuration rollout for A/B testing.
//!
//! This example shows how to:
//! - Set up a canary configuration
//! - Gradually increase rollout percentage
//! - Use consistent hashing for user-specific routing
//! - Promote or rollback canary changes
//!
//! Run with: cargo run --example gradual_rollout --features gradual-rollout

use hotswap_config::features::GradualRolloutExt;
use hotswap_config::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(default)]
struct AppConfig {
    version: String,
    api_endpoint: String,
    timeout_ms: u32,
    experimental_features: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: "v1.0".to_string(),
            api_endpoint: "https://api.example.com/v1".to_string(),
            timeout_ms: 5000,
            experimental_features: false,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Gradual Rollout (A/B Testing) Example ===\n");

    // Build parent configuration (v1.0) with a registered validator so
    // `try_set_canary` and `promote` validate candidates automatically.
    let config = HotswapConfig::builder()
        .with_env_overrides("HOTSWAP_GRADUAL_EXAMPLE", "__")
        .with_validation(|cfg: &AppConfig| {
            if cfg.timeout_ms == 0 {
                return Err(ValidationError::invalid_field(
                    "timeout_ms",
                    "must be greater than 0",
                ));
            }
            if !cfg.api_endpoint.starts_with("https://") {
                return Err(ValidationError::invalid_field(
                    "api_endpoint",
                    "must use https://",
                ));
            }
            Ok(())
        })
        .build::<AppConfig>()
        .await?;

    // Enable gradual rollout linked to the validated parent config
    let rollout = config.enable_gradual_rollout();
    println!("Enabled gradual rollout\n");

    println!("Stable configuration (v1.0):");
    let stable = rollout.get_stable().await;
    print_config(&stable);
    println!();

    // Demonstrate pre-flight validation on `try_set_canary`
    println!("--- Attempting invalid canary (timeout_ms = 0) ---");
    let invalid_canary = AppConfig {
        version: "v2.0-invalid".to_string(),
        api_endpoint: "https://api.example.com/v2".to_string(),
        timeout_ms: 0,
        experimental_features: true,
    };
    if let Err(err) = rollout.try_set_canary(Arc::new(invalid_canary), 10).await {
        println!("Rejected invalid canary as expected: {}", err);
        println!(
            "Has canary after rejection: {}\n",
            rollout.has_canary().await
        );
    }

    // Set up valid canary configuration (v2.0)
    println!("--- Setting up canary (v2.0) with 10% rollout ---");
    let canary_config = AppConfig {
        version: "v2.0".to_string(),
        api_endpoint: "https://api.example.com/v2".to_string(),
        timeout_ms: 3000,
        experimental_features: true,
    };
    rollout.try_set_canary(Arc::new(canary_config), 10).await?;

    println!("Canary configuration (v2.0):");
    print_config(&rollout.get_canary().await.unwrap());
    println!("Rollout: {}%\n", rollout.get_percentage().await);

    // Simulate random requests
    println!("--- Simulating 20 random requests ---");
    let mut stable_count = 0;
    let mut canary_count = 0;

    for i in 1..=20 {
        let cfg = rollout.get(None).await;
        if cfg.version == "v1.0" {
            stable_count += 1;
            println!("Request {}: Stable (v1.0)", i);
        } else {
            canary_count += 1;
            println!("Request {}: Canary (v2.0)", i);
        }
    }
    println!(
        "\nResults: {} stable, {} canary (~{}% canary)\n",
        stable_count,
        canary_count,
        (canary_count * 100) / 20
    );

    // Simulate user-specific routing with consistent hashing
    println!("--- Demonstrating consistent hashing ---");
    let test_users = vec!["user123", "user456", "user789"];

    for user in &test_users {
        let cfg1 = rollout.get(Some(user)).await;
        let cfg2 = rollout.get(Some(user)).await;
        let cfg3 = rollout.get(Some(user)).await;

        println!(
            "User '{}': {} (consistent across 3 requests: {})",
            user,
            cfg1.version,
            cfg1.version == cfg2.version && cfg2.version == cfg3.version
        );
    }
    println!();

    // Increase rollout percentage
    println!("--- Increasing rollout to 25% ---");
    rollout.increase_percentage(15).await;
    println!("New rollout: {}%\n", rollout.get_percentage().await);

    // Simulate more requests
    println!("Simulating 20 more requests:");
    stable_count = 0;
    canary_count = 0;

    for _ in 1..=20 {
        let cfg = rollout.get(None).await;
        if cfg.version == "v1.0" {
            stable_count += 1;
        } else {
            canary_count += 1;
        }
    }
    println!(
        "Results: {} stable, {} canary (~{}% canary)\n",
        stable_count,
        canary_count,
        (canary_count * 100) / 20
    );

    // Increase to 50%
    println!("--- Increasing rollout to 50% ---");
    rollout.increase_percentage(25).await;
    println!("New rollout: {}%\n", rollout.get_percentage().await);

    // Simulate more requests
    println!("Simulating 20 more requests:");
    stable_count = 0;
    canary_count = 0;

    for _ in 1..=20 {
        let cfg = rollout.get(None).await;
        if cfg.version == "v1.0" {
            stable_count += 1;
        } else {
            canary_count += 1;
        }
    }
    println!(
        "Results: {} stable, {} canary (~{}% canary)\n",
        stable_count,
        canary_count,
        (canary_count * 100) / 20
    );

    // Promote canary to stable
    println!("--- Promoting canary to stable ---");
    rollout.promote().await?;
    println!("Canary promoted! All traffic now on v2.0\n");

    println!("New stable configuration:");
    let stable = rollout.get_stable().await;
    print_config(&stable);
    println!("Has canary: {}", rollout.has_canary().await);
    println!("Rollout: {}%\n", rollout.get_percentage().await);

    // Verify all requests get v2.0
    println!("--- Verifying all requests use v2.0 ---");
    for i in 1..=5 {
        let cfg = rollout.get(None).await;
        println!("Request {}: {}", i, cfg.version);
    }
    println!();

    // Demonstrate rollback scenario
    println!("--- Demonstrating rollback scenario ---");
    println!("Setting new canary v2.1 with 30% rollout");

    let canary_v2_1 = AppConfig {
        version: "v2.1".to_string(),
        api_endpoint: "https://api.example.com/v2.1".to_string(),
        timeout_ms: 2000,
        experimental_features: true,
    };
    rollout.try_set_canary(Arc::new(canary_v2_1), 30).await?;
    println!("Canary v2.1 set\n");

    // Simulate discovering an issue
    println!("Issue detected in canary! Rolling back...");
    rollout.rollback_canary().await;
    println!("Canary rolled back\n");

    println!("Current state:");
    let stable = rollout.get_stable().await;
    println!("  Stable: {}", stable.version);
    println!("  Has canary: {}", rollout.has_canary().await);
    println!("  Rollout: {}%", rollout.get_percentage().await);

    println!("\n\nExample complete!");
    println!("\nKey benefits of gradual rollout:");
    println!("  - Test new configurations with limited traffic");
    println!("  - Consistent hashing ensures same users get same config");
    println!("  - Gradually increase rollout percentage");
    println!("  - Promote to stable or rollback based on results");
    println!("  - Perfect for A/B testing and canary deployments");

    Ok(())
}

fn print_config(cfg: &AppConfig) {
    println!("  Version: {}", cfg.version);
    println!("  API Endpoint: {}", cfg.api_endpoint);
    println!("  Timeout: {}ms", cfg.timeout_ms);
    println!("  Experimental: {}", cfg.experimental_features);
}
