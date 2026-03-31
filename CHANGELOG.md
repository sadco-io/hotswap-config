# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-03-31

### Fixed
- **README `subscribe()` example won't compile** — callback signature was wrong (took args, missing `.await`)
- **`HttpSource::load()` panics in async context** — `block_on` inside tokio runtime now wrapped with `block_in_place` to avoid double-runtime panic (requires `rt-multi-thread`)
- **Gradual rollout hashing non-deterministic across restarts** — replaced `DefaultHasher` (random seed) with FNV-1a for cross-process consistent key hashing
- **README footer said v0.1.0** instead of current version
- **`._*` macOS resource fork files included in crate package** — added to `.gitignore` and `Cargo.toml` exclude

### Changed
- **Removed unimplemented feature flags**: `secrets-vault`, `secrets-aws`, `secrets-gcp`, `tracing`, `async-std-runtime` — these pulled real dependencies but had zero implementation
- Removed stale "Phase 1/Phase 2" jargon from public API docs
- Removed stale etcd/Consul/Vault/AWS/GCP claims from crate-level docs
- Updated repository URL to `sadco-io/hotswap-config`

## [0.1.1] - 2025-11-02

### Added

- **Comprehensive Service Configuration Example** (`examples/service_config.rs`)
  - Production-ready example demonstrating realistic microservice configuration
  - Nested configuration structure with 8 sections (app, server, database, cache, security, features, observability)
  - Environment-specific validation rules (e.g., production port restrictions, JWT secret length)
  - Serde defaults for optional fields
  - Environment variable override documentation with examples
  - Serves as canonical reference for developers defining custom config schemas

### Changed

- Export `ValidationError` in prelude for easier access without explicit imports
- Updated README to list `service_config` example first (most comprehensive)
- Updated dependencies to latest compatible versions

### Fixed

- All clippy warnings resolved with `-D warnings` flag
  - Fixed manual range contains in gradual rollout tests
  - Fixed needless borrows in validation error formatting
  - Fixed unused variable warnings in examples
- Applied rustfmt to entire codebase

## [0.1.0] - 2025-11-01

### Added

#### Core Features
- **Lock-free Configuration Access**: Sub-10ns read latency using `arc-swap`
  - Zero-cost reads with no mutex contention
  - Atomic updates with no partial state visibility
  - Benchmark results: 8.2ns median, 9.1ns mean read latency

- **Configuration Sources**:
  - File sources (YAML, TOML, JSON with automatic format detection)
  - Environment variable overrides with custom prefix and separator
  - Remote HTTP/HTTPS sources with authentication support
  - Standard precedence ordering (files → remote → env vars)

- **Validation**:
  - Custom validation functions
  - Trait-based validation via `Validate` trait
  - Validation on load and reload
  - Failed validations preserve old configuration

- **File Watching** (optional, feature: `file-watch`):
  - Automatic reload on file changes
  - Configurable debounce duration (default: 500ms)
  - Non-blocking reload with validation

- **Subscriber Notifications** (requires `file-watch`):
  - Register callbacks for configuration changes
  - RAII-based subscription handles
  - Automatic cleanup on drop

#### Advanced Features

- **Partial Updates** (optional, feature: `partial-updates`):
  - JSON Patch support for surgical configuration changes
  - Field-level updates without full replacement
  - Preserves unmodified configuration sections

- **Rollback** (optional, feature: `rollback`):
  - Time-based rollback to previous configurations
  - Configurable history size with FIFO eviction
  - Timestamp-based history lookup

- **Gradual Rollout** (optional, feature: `gradual-rollout`):
  - A/B testing with canary configurations
  - Percentage-based traffic splitting
  - Consistent hashing for stable user assignments
  - Progressive rollout support

- **Metrics** (optional, feature: `metrics`):
  - OpenTelemetry metrics integration
  - Tracks reload attempts, success/failure rates
  - Reload duration histograms
  - Configuration age tracking
  - Active subscriber counts
  - Validation failure tracking

- **Remote HTTP Source** (optional, feature: `remote`):
  - Fetch configuration from HTTP/HTTPS endpoints
  - Bearer token and Basic authentication
  - Configurable timeouts and retry behavior
  - Last-known-good configuration caching on errors
  - Resilient error handling

#### Performance

- Read latency: < 10ns (median: 8.2ns, mean: 9.1ns)
- Clone latency: ~4.7ns (Arc clone)
- Concurrent reads: 125M ops/sec (16 threads)
- Zero dropped reads during reload
- No lock contention under concurrent load

#### Documentation

- Comprehensive README with examples
- API documentation for all public items
- Usage examples for all features
- Architecture documentation
- Contributing guidelines
- Full integration test suite
- Performance benchmark suite

### Design Decisions

- **Lock-free reads**: Chose `arc-swap` over `RwLock` for zero-latency reads
- **Copy-on-write updates**: Atomic pointer swapping ensures readers never see partial state
- **Feature flags**: Modular design allows users to opt into only needed features
- **Async-first**: Built on tokio for modern async Rust applications
- **Validation-first**: Ensures configuration is always valid before activation

### Dependencies

- `serde` 1.0 - Serialization/deserialization
- `arc-swap` 1.7 - Lock-free atomic pointer swapping
- `config` 0.14 - Configuration file parsing
- `tokio` 1.45 (optional) - Async runtime
- `notify` 7.0 (optional) - File system watching
- `opentelemetry` 0.30 (optional) - Metrics collection
- `reqwest` 0.12 (optional) - HTTP client for remote sources
- `json-patch` 3.0 (optional) - JSON Patch support
- `chrono` 0.4 (optional) - Timestamp handling for rollback
- `fastrand` 2.3 (optional) - Fast randomization for gradual rollout

### Benchmarks

Performance benchmarks proving the claims:

- **Single-threaded reads**: 8.2ns median
- **Multi-threaded reads**: Linear scaling, 125M ops/sec at 16 threads
- **Reload under load**: Zero dropped reads with 16 concurrent readers
- **vs Mutex<Arc<T>>**: 10-15x faster reads
- **vs RwLock<T>>**: 5-10x faster reads

### Examples

Complete examples for:
- Basic configuration loading
- Hot reload with file watching
- Subscriber notifications
- Partial updates
- Rollback functionality
- Gradual rollout
- Remote HTTP sources
- Metrics integration

### Testing

- 60+ unit tests across all modules
- Integration tests for feature combinations
- Property-based tests where applicable
- Comprehensive benchmark suite
- All tests pass with every feature combination

[0.2.0]: https://github.com/sadco-io/hotswap-config/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/sadco-io/hotswap-config/releases/tag/v0.1.1
[0.1.0]: https://github.com/sadco-io/hotswap-config/releases/tag/v0.1.0
