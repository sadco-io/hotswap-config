# hotswap-config

[![Crates.io](https://img.shields.io/crates/v/hotswap-config.svg)](https://crates.io/crates/hotswap-config)
[![Documentation](https://docs.rs/hotswap-config/badge.svg)](https://docs.rs/hotswap-config)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.87.0-blue)](Cargo.toml)

A high-performance, hot-reloadable configuration store with **wait-free reads** and **transactional updates** for Rust applications.

## Core Features

- **Wait-free reads** via atomic pointer swap (`ArcSwap` pattern) - readers never block
- **File watching** (cross-platform, `notify` crate) with automatic reload
- **Subscribers**: Register callbacks for async/sync notifications on config changes
- **Validation + atomic rollback**: Invalid configs are rejected; readers never see partial state

## Advanced Features (Optional)

- **Partial updates**: RFC 6902 JSON Patch for surgical field changes (feature: `partial-updates`)
- **Versioned history**: Point-in-time rollback with timestamps (feature: `rollback`)
- **Gradual rollout / A/B testing**: Percentage-based, key-scoped canary deployment (feature: `gradual-rollout`)
- **Remote HTTP sources**: Fetch config from HTTP(S) endpoints with Bearer/Basic auth (feature: `remote`)
- **OpenTelemetry metrics**: Track reload success/failures, latency, config age (feature: `metrics`)

## Performance (Benchmarked)

**Test System:** Apple MacBook Pro (M3 Pro, 12-core, 18GB unified memory), macOS 26.0.1, Rust 1.87.0
**Build:** `cargo bench --release` (LTO=true, opt-level=3, codegen-units=1)
**Methodology:** Criterion 0.5, 100 samples, warm L3 cache

| Metric | Result | Notes |
|--------|--------|-------|
| **Read latency (median)** | **7.16 ns/read** | Single-threaded, warm cache |
| **Read latency (mean)** | 7.43 ns/read | Sub-10ns target achieved |
| **Throughput (1 thread)** | 206M reads/sec | = 206 million operations/sec |
| **Throughput (16 threads)** | 229M reads/sec total | = 14.4M reads/sec/thread |
| **Config clone** | 7.86 ns | Cheap `Arc` clone |
| **Reload under load** | 0 dropped reads | Zero downtime validated |

> **Readers are wait-free:** `ArcSwap::load()` is a single atomic read. Writers build new config off-to-the-side, validate, then atomically swap. Old readers continue using the previous `Arc` until dropped.

See [`benches/README.md`](benches/README.md) for full methodology, CPU governor settings, and raw criterion reports.

## Quick Start

Add to `Cargo.toml`:

```toml
[dependencies]
hotswap-config = "0.2"
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "1.0", features = ["full"] }
```

**10-line working example:**

```rust
use hotswap_config::prelude::*;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    server_port: u16,
    feature_flag: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load config with file watching (auto-reloads on change)
    let config = HotswapConfig::builder()
        .with_file("config/default.yaml")
        .with_env_overrides("APP", "__")  // APP_SERVER_PORT=8080 overrides file
        .with_validation(|cfg: &AppConfig| {
            if cfg.server_port < 1024 {
                return Err(ValidationError::invalid_field("server_port", "must be >= 1024"));
            }
            Ok(())
        })
        .build::<AppConfig>()
        .await?;

    // Wait-free reads (no locks!)
    let cfg = config.get();  // Returns Arc<AppConfig>
    println!("Server starting on port {}", cfg.server_port);

    // Subscribe to changes
    let _subscription = config.subscribe(|| {
        println!("Config reloaded!");
    }).await;

    // Config automatically reloads when config/default.yaml changes
    // Invalid configs are rejected; old config stays active

    Ok(())
}
```

**With partial updates (JSON Patch):**

```rust
// Feature: partial-updates
config.update_field("/feature_flag", true).await?;  // Atomic update with validation
```

## Feature Flags

| Feature | Description | Dependencies |
|---------|-------------|--------------|
| `file-watch` | Auto-reload on file changes (default) | `notify`, `tokio` |
| `validation` | Config validation trait (default) | - |
| `yaml` | YAML file format support | `serde_yaml` |
| `toml` | TOML file format support | `toml` |
| `json` | JSON file format support | `serde_json` |
| `all-formats` | Enable all formats | - |
| `partial-updates` | JSON Patch (RFC 6902) | `json-patch`, `tokio` |
| `rollback` | Version history & rollback | `chrono`, `tokio` |
| `gradual-rollout` | A/B testing & canary | `fastrand`, `tokio` |
| `remote` | HTTP(S) config sources | `reqwest`, `tokio` |
| `metrics` | OpenTelemetry metrics | `opentelemetry` |

**Default features:** `file-watch`, `validation`

Enable features in `Cargo.toml`:

```toml
[dependencies]
hotswap-config = { version = "0.2", features = ["partial-updates", "rollback", "yaml"] }
```

## Configuration Sources & Precedence

Config sources are merged by priority (highest wins):

1. **Environment variables** (priority: 300) - `APP_SERVER__PORT=8080`
2. **Remote HTTP sources** (priority: 250, if enabled)
3. **Environment-specific files** (priority: 110+) - `config/production.yaml`
4. **Default files** (priority: 100) - `config/default.yaml`

### Supported Formats

- **YAML** (.yaml, .yml) - Feature: `yaml`
- **TOML** (.toml) - Feature: `toml`
- **JSON** (.json) - Feature: `json`

Format detected automatically by file extension.

## Safety & Failure Modes

### Validation

- **When:** Before initial load and before every update/reload
- **What:** Custom validation functions (`Fn(&T) -> Result<(), ValidationError>`)
- **Failure:** Validation errors reject the update; readers continue using old config
- **Guarantee:** Readers **never** see invalid or partial config state

### Remote HTTP Sources (feature: `remote`)

- **TLS:** Supports HTTPS with native TLS roots (`rustls`, `native-certs`)
- **Authentication:** Bearer token or Basic auth
- **Retry/backoff:** On network errors, keeps last-known-good config
- **Security:** Does **not** currently support certificate pinning or config signatures (planned for v0.2.0)

### File Watching

- **Cross-platform:** Uses `notify` crate (inotify/kqueue/FSEvents)
- **Debouncing:** 500ms default (configurable) to avoid rapid reloads
- **Error handling:** File watch errors log but don't crash; manual `reload()` still works

## Testing & QA

- ✅ **80+ unit & integration tests** - All feature combinations covered
- ✅ **Concurrency tests** - Validated with Tokio test framework
- ✅ **Property-based tests** - Using `proptest` for validation logic
- ✅ **CI Matrix** - GitHub Actions on Linux, macOS (ARM64)
- ✅ **6 runnable examples** - `examples/` directory with full scenarios

### Running Tests

```bash
# All tests with all features
cargo test --all-features

# Specific feature combinations
cargo test --features yaml,validation
cargo test --features "partial-updates,rollback"

# No default features
cargo test --no-default-features
```

### Running Examples

```bash
# Service configuration (comprehensive example with validation)
cargo run --example service_config --features yaml

# Hot reload demonstration
cargo run --example hot_reload --features yaml

# Subscriber notifications
cargo run --example subscribers --features yaml

# Partial updates
cargo run --example partial_updates --features "yaml,partial-updates"

# Rollback demonstration
cargo run --example rollback --features "yaml,rollback"

# Gradual rollout
cargo run --example gradual_rollout --features "yaml,gradual-rollout"

# Remote HTTP source
cargo run --example remote_config --features "remote,yaml"
```

## Documentation

- **API Documentation**: [docs.rs/hotswap-config](https://docs.rs/hotswap-config)
- **Examples**: [`examples/`](examples/) directory
- **Design Document**: [`DESIGN.md`](DESIGN.md)
- **Contributing**: [`CONTRIBUTING.md`](CONTRIBUTING.md)
- **Changelog**: [`CHANGELOG.md`](CHANGELOG.md)

## Comparison with Other Crates

| Feature | hotswap-config | config | figment | confy |
|---------|---------------|--------|---------|-------|
| Wait-free reads | ✅ (ArcSwap) | ❌ | ❌ | ❌ |
| Hot reload | ✅ | ❌ | ❌ | ❌ |
| File watching | ✅ | ❌ | ❌ | ❌ |
| Validation | ✅ | Limited | ✅ | ❌ |
| Atomic updates | ✅ | ❌ | ❌ | ❌ |
| Rollback | ✅ | ❌ | ❌ | ❌ |
| Partial updates | ✅ | ❌ | ❌ | ❌ |
| Remote sources | ✅ | ❌ | Limited | ❌ |
| Metrics | ✅ | ❌ | ❌ | ❌ |
| Type-safe | ✅ | ✅ | ✅ | ✅ |

## Platform Support

- **Platforms:** Linux, macOS, Windows (via `notify` crate)
- **MSRV:** Rust 1.87.0 (edition 2024)
- **no_std:** Not supported (requires `std::sync::Arc`, tokio runtime)

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

## Acknowledgments

- Built on the excellent [`arc-swap`](https://crates.io/crates/arc-swap) crate for lock-free atomic updates
- Configuration parsing via [`config`](https://crates.io/crates/config) crate
- File watching via [`notify`](https://crates.io/crates/notify) crate
- Pattern proven at scale in production microservices handling 1M+ permission checks/second

---

**Status:** v0.2.0 - Production-ready, API stable

Built with ❤️ by [Daniel Curtis](https://github.com/danielrcurtis)
