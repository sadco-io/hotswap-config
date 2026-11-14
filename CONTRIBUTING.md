# Contributing to hotswap-config

Thank you for your interest in contributing to hotswap-config! This document provides guidelines and information for contributors.

## Code of Conduct

This project follows a Code of Conduct. By participating, you are expected to uphold this code. Please be respectful and professional in all interactions.

## Getting Started

### Prerequisites

- Rust 1.87.0 or later
- Cargo
- Familiarity with async Rust (tokio)
- Basic understanding of configuration management patterns

### Development Setup

1. Fork and clone the repository:
```bash
git clone https://github.com/sadco-io/hotswap-config.git
cd hotswap-config
```

2. Build the project:
```bash
cargo build --all-features
```

3. Run tests:
```bash
cargo test --all-features
```

4. Run benchmarks (optional):
```bash
cargo bench
```

## Development Workflow

### Making Changes

1. Create a new branch for your feature or fix:
```bash
git checkout -b feature/your-feature-name
```

2. Make your changes, following the code style guidelines below

3. Add tests for your changes

4. Ensure all tests pass:
```bash
cargo test --all-features
cargo clippy --all-features -- -D warnings
cargo fmt --check
```

5. Commit your changes with a descriptive message

6. Push to your fork and create a pull request

### Code Style

- Use `cargo fmt` to format all code
- Use `cargo clippy` to catch common mistakes
- Follow Rust API design guidelines
- Document all public APIs with doc comments
- Include examples in doc comments where appropriate

### Testing Requirements

- All new features must have unit tests
- Integration tests for feature combinations
- Benchmarks for performance-critical code
- Test with all relevant feature flags

### Feature Flags

When adding new features:
- Use appropriate feature flags
- Document feature requirements
- Ensure the feature compiles standalone
- Test feature combinations

### Documentation

- Update README.md if adding user-facing features
- Add doc comments to all public items
- Include examples in doc comments
- Update CHANGELOG.md

## Pull Request Process

1. **Before Submitting**:
   - Run `cargo test --all-features` (all tests must pass)
   - Run `cargo clippy --all-features -- -D warnings` (no warnings)
   - Run `cargo fmt` (code must be formatted)
   - Update documentation as needed
   - Add an entry to CHANGELOG.md

2. **PR Description**:
   - Clearly describe the problem and solution
   - Link to any relevant issues
   - Include examples of usage if applicable
   - Note any breaking changes

3. **Review Process**:
   - Address review feedback promptly
   - Keep PRs focused and reasonably sized
   - Be open to suggestions and improvements

## Types of Contributions

### Bug Reports

When filing a bug report, include:
- Rust version
- Operating system
- Minimal reproduction case
- Expected vs actual behavior
- Relevant error messages

### Feature Requests

For feature requests, provide:
- Use case and motivation
- Proposed API design (if applicable)
- Alternatives considered
- Impact on existing code

### Code Contributions

We welcome:
- Bug fixes
- New features (discuss in issues first for major features)
- Performance improvements
- Documentation improvements
- Test coverage improvements

## Project Structure

```
hotswap-config/
├── src/
│   ├── core/          # Core configuration handling
│   ├── sources/       # Configuration sources (file, env, remote)
│   ├── features/      # Optional advanced features
│   ├── metrics/       # OpenTelemetry metrics (optional)
│   └── notify/        # File watching and subscribers (optional)
├── examples/          # Usage examples
├── tests/             # Integration tests
└── benches/           # Performance benchmarks
```

## Building Documentation

```bash
cargo doc --all-features --open
```

## Running Benchmarks

```bash
cargo bench --all-features
```

To run specific benchmarks:
```bash
cargo bench --bench read_performance
```

## Feature Implementation Guidelines

### Adding a New Source

1. Create module in `src/sources/`
2. Implement `ConfigSource` trait
3. Add feature flag if optional
4. Add tests
5. Add example in `examples/`
6. Document in README.md

### Adding a New Feature

1. Create module in `src/features/`
2. Define extension trait if wrapping `HotswapConfig`
3. Add feature flag
4. Add comprehensive tests
5. Add example
6. Benchmark if performance-critical
7. Document in README.md

## Release Process

1. Update version in `Cargo.toml`
2. Update `CHANGELOG.md`
3. Update `README.md` if needed
4. Create git tag
5. Publish to crates.io

## Questions?

- Open an issue for questions
- Check existing issues and discussions
- Refer to the README.md for usage examples

## License

By contributing, you agree that your contributions will be licensed under the same terms as the project (MIT OR Apache-2.0).

Thank you for contributing to hotswap-config!
