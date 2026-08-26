# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.2] - 2026-08-26

### Changed

- **`json-patch` 3.0 -> 4.2.** Internal only. The crate is used at three call sites
  in `src/features/partial.rs` (`json_patch::Patch`, `json_patch::patch`) behind the
  `partial-updates` feature, and no `json_patch` type appears in this crate's public
  API, so the major bump is not a breaking change for consumers. Deduplicates
  `jsonptr` 0.6 -> 0.7 and drops `thiserror` 1.x from the tree.
- **`criterion` 0.5 -> 0.8** (dev-dependency, benches only). criterion 0.6 deprecated
  its `criterion::black_box` re-export in favour of `std::hint::black_box`; because the
  lint job runs `clippy --all-features --all-targets -- -D warnings`, the deprecation
  was a build failure, not a warning. `benches/read_performance.rs` now imports
  `black_box` from `std::hint` -- the same function, at its new path. No benchmark
  logic changed. criterion 0.8.2 declares `rust-version = 1.86`, below this crate's
  1.87 floor, which is harmless: the `msrv` job is `cargo build --all-features` and
  does not compile dev-dependencies.

## [0.2.1] - 2026-08-25

### Fixed

- **`serde_yaml` and `toml` were declared but never used.** Nothing in `src/`, `tests/`,
  `examples/` or `benches/` referenced either crate -- all format parsing goes through
  `config`. Both are removed. This drops `serde_yaml`, which upstream archived and
  publishes as `0.9.34+deprecated`, out of every downstream dependency tree.
- **The `yaml` / `toml` / `json` / `all-formats` feature flags did nothing.** They gated the
  two unused dependencies above while `config` was built unconditionally with all three
  parsers. They now forward to `config`'s own parsers (`config/yaml` etc.) and are enabled
  by default via `all-formats`, so the **default feature set behaves exactly as before**.
  Consumers building with `default-features = false` must now name the formats they want --
  in exchange, they get a build that pulls no format parsers at all.
- **Seven targets could not compile under some supported feature combination.**
  `remote_config`, `gradual_rollout`, `partial_updates` and `rollback` use feature-gated
  APIs; `hot_reload`, `subscribers` and `service_config` call `subscribe()`, which lives
  behind `file-watch`; and the `basic_loading` / `full_integration` suites plus
  `sources::file::tests::test_load_yaml_file` load YAML fixtures, which since this release
  arrive via the `yaml` feature rather than unconditionally. Each now declares
  `required-features`, or is `#[cfg]`-gated. Three of these were found by the new CI job
  after the first push -- they had never been exercised.
- **Four of the seven examples could not compile.** `remote_config`, `gradual_rollout`,
  `partial_updates` and `rollback` use feature-gated APIs but were not declared with
  `required-features`, so `cargo build --examples` failed on the default feature set. Each
  now declares the feature it needs.
- `cargo fmt` drift in `src/sources/remote.rs`.

### Security

- **`notify` 7.0 -> 8.2 clears RUSTSEC-2024-0384.** `notify` 7 depends on `notify-types` 1,
  which pulls the unmaintained `instant` crate; `notify-types` 2 replaced it with `web-time`.
  This was **not** caught in the first pass of this audit, which only matched *direct*
  dependencies against the advisory database -- `instant` is transitive. `cargo deny check`
  finds it immediately, which is the argument for the new CI job rather than a manual sweep.
  The bump is a drop-in for this crate: zero code changes, all 80 tests pass. It was
  originally scheduled for 0.3.0 as a routine major; clearing an advisory moves it here.

### Changed

- **`reqwest` no longer links OpenSSL by default.** The `remote` feature pulled `reqwest`
  with its default TLS backend, which meant `native-tls` -> `openssl-sys` and a hard
  requirement on `libssl-dev` + `pkg-config` on the build host -- `cargo build
  --all-features` could not complete without them. `remote` now uses rustls and needs no
  system libraries at all.

  Platform-certificate-store users are **not** left behind: the new
  **`remote-native-tls`** feature compiles reqwest's native-tls backend and selects it at
  runtime, for deployments that depend on corporate roots or OS-managed trust. Both
  backends are covered independently in CI. If you were relying on the platform trust
  store, add `features = ["remote-native-tls"]`; no code change is needed.
- Dependency floors raised to current, all semver-compatible: `arc-swap` `1.7` -> `1.9`,
  `fastrand` `2.3` -> `2.5`, dev `tempfile` `3.14` -> `3.27`, dev `proptest` `1.6` -> `1.11`.

### Added

- CI (`.github/workflows/ci.yml`): test on stable + beta, an MSRV job pinned to the declared
  1.87.0, `fmt` + `clippy -D warnings`, `cargo doc -D warnings`, and `cargo deny check`.
- `deny.toml` for advisory, license and source auditing.

### Notes

- Deferred to 0.3.0 (all breaking): `config` `0.14` -> `0.15` (`config::Value` is in this
  crate's public API), `opentelemetry` `0.30` -> `0.32` (`Meter`/`Counter`/`Gauge`/`Histogram`
  are held in `ConfigMetrics`), `reqwest` `0.12` -> `0.13`, `json-patch` `3` -> `4`,
  dev `criterion` `0.5` -> `0.8`.
- `cargo deny check` passes clean on all four checks as of this release.

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
