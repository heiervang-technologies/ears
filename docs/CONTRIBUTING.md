# Contributing to ears

## Development Setup

### Prerequisites

1. **Rust toolchain** (stable)
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **System dependencies** (see [INSTALL.md](INSTALL.md))

3. **Whisper.cpp server** running locally (for integration testing)

### Clone and Build

```bash
git clone https://github.com/heiervang-technologies/ears
cd ears
cargo build
cargo test
```

## Code Style

- `cargo fmt` before committing
- `cargo clippy -- -D warnings` with no warnings
- `anyhow` for application errors, `thiserror` for library errors
- Doc comments on all public APIs

## Testing

```bash
cargo test                          # All tests
cargo test config                   # Specific module
cargo test -- --nocapture           # With output
cargo test -- --test-threads=1      # Sequential
```

Tests use `tempfile` for temp dirs, `wiremock` for HTTP mocking, `serial_test` for env var tests.

## Commit Messages

[Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation
- `refactor:` - Code refactoring
- `test:` - Test changes
- `chore:` - Maintenance

## Pull Request Process

1. Create a feature branch
2. Write code + tests + docs
3. Run `cargo fmt && cargo clippy && cargo test`
4. Submit PR with clear description
5. CI must pass (formatting, clippy, tests)

## Release Process

This project uses [release-plz](https://release-plz.dev/) for automated releases. See [RELEASING.md](RELEASING.md).

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for system design.

## License

By contributing, you agree your contributions will be licensed under MIT.
